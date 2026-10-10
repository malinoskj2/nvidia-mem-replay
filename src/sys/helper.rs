use crate::{
    config::{self, Config, Volume},
    filesystem::MAX_RECOVERY_SNAPSHOT_BYTES,
    sys::nvidia::Redirect,
    telemetry::{
        MAX_TELEMETRY_FRAME_BYTES, Sample, TELEMETRY_TIMEOUT, TelemetryError, decode_sample,
    },
};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use thiserror::Error;

mod diagnostics;
use diagnostics::Diagnostics;

const READY_TIMEOUT: Duration = Duration::from_secs(15);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const RECOVERY_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const FORCED_TERMINATION_TIMEOUT: Duration = Duration::from_secs(1);
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MEMEFS_EXECUTABLE: &str = "memefs-x64.exe";
const STOP_COMMAND: &[u8] = b"stop\n";

#[derive(Debug, Error)]
pub(crate) enum HelperError {
    #[error("RAM filesystem helper I/O: {0}")]
    Io(#[from] io::Error),
    #[error("RAM storage location {0} is already in use")]
    Occupied(String),
    #[error("RAM filesystem did not become ready within 15 seconds")]
    Timeout,
    #[error("RAM filesystem stopped unexpectedly with {0}")]
    Exited(ExitStatus),
    #[error("RAM filesystem helper exited with {0}")]
    FailedExit(ExitStatus),
    #[error("RAM filesystem helper exceeded its shutdown deadline and was forcibly terminated")]
    ForcedTermination,
    #[error("RAM filesystem helper did not exit after forced termination")]
    TerminationTimeout,
    #[error("recovery snapshot JSON: {0}")]
    Snapshot(#[from] serde_json::Error),
    #[error("recovery snapshot exceeds 64 KB")]
    SnapshotOversized,
    #[error("helper telemetry: {0}")]
    Telemetry(#[source] Arc<TelemetryError>),
    #[error("telemetry lock failed")]
    TelemetryLock,
    #[error("helper stopped publishing telemetry")]
    TelemetryStale,
    #[error("telemetry reader thread failed")]
    TelemetryReader,
    #[error("{error}{diagnostics}")]
    Diagnostics {
        #[source]
        error: Box<Self>,
        diagnostics: String,
    },
}

#[derive(Default)]
struct Inbox {
    latest: Option<(Sample, Instant)>,
    error: Option<Arc<TelemetryError>>,
}

pub(crate) struct ShutdownReport {
    pub(crate) sample: Option<Sample>,
    pub(crate) result: Result<(), HelperError>,
    pub(crate) exited: bool,
}

pub(crate) struct Helper {
    child: Child,
    input: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
    inbox: Arc<Mutex<Inbox>>,
    diagnostics: Diagnostics,
    shutdown_timeout: Duration,
}

/// Fails when something already occupies the mount point. A mount-point directory left behind
/// by a helper that died (`WinFsp` normally removes it on unmount) is a dangling reparse point
/// that cannot be listed; it is removed so the volume can be mounted there again.
fn ensure_mount_point_free(mount: &str) -> Result<(), HelperError> {
    let is_drive = config::is_drive_letter(mount);
    let probe = if is_drive {
        format!("{mount}\\")
    } else {
        mount.to_owned()
    };
    match std::fs::symlink_metadata(&probe) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(metadata)
            if !is_drive
                && metadata.file_type().is_symlink()
                && std::fs::read_dir(&probe).is_err() =>
        {
            std::fs::remove_dir(&probe)?;
            Ok(())
        }
        Ok(_) => Err(HelperError::Occupied(mount.to_owned())),
    }
}

impl Helper {
    pub(crate) fn start(config: &Config, redirect: &Redirect) -> Result<Self, HelperError> {
        let volume = config.volume();
        ensure_mount_point_free(&volume.mount)?;

        let mut command = Command::new(std::env::current_exe()?);
        command
            .args([
                "filesystem",
                "--mount",
                &volume.mount,
                "--memory-limit-mb",
                &config.memory_limit_mb.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }

        Self::wait_ready(command, &volume, Some(redirect))
    }

    #[cfg(windows)]
    pub(crate) fn start_memefs(volume: &Volume) -> Result<Self, HelperError> {
        use std::os::windows::process::CommandExt;
        let executable = std::env::current_exe()?.with_file_name(MEMEFS_EXECUTABLE);
        let mut command = Command::new(executable);
        command
            .args(["-m", &volume.mount, "-s", &volume.limit_bytes.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x0800_0000);

        Self::wait_ready(command, volume, None)
    }

    fn wait_ready(
        mut command: Command,
        volume: &Volume,
        redirect: Option<&Redirect>,
    ) -> Result<Self, HelperError> {
        let snapshot = redirect.map(serde_json::to_vec).transpose()?;
        if snapshot
            .as_ref()
            .is_some_and(|bytes| bytes.len() >= MAX_RECOVERY_SNAPSHOT_BYTES)
        {
            return Err(HelperError::SnapshotOversized);
        }

        let mut helper = Self::from_child(command.spawn()?)?;
        if let Err(error) = helper.await_ready(&volume.target(), snapshot.as_deref()) {
            let _ = helper.stop();
            return Err(helper.diagnostics.attach(error));
        }

        Ok(helper)
    }

    fn await_ready(&mut self, target: &str, snapshot: Option<&[u8]>) -> Result<(), HelperError> {
        if let Some(snapshot) = snapshot {
            // The first telemetry frame acknowledges receipt of this exact snapshot.
            self.shutdown_timeout = RECOVERY_SHUTDOWN_TIMEOUT;
            let input = self
                .input
                .as_mut()
                .ok_or_else(|| io::Error::other("missing helper stdin"))?;
            input.write_all(snapshot)?;
            input.write_all(b"\n")?;
        }

        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if self.sample()?.is_some() {
                std::fs::create_dir_all(target)?;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(HelperError::Timeout);
            }
            thread::sleep(READY_POLL_INTERVAL);
        }
    }

    pub(crate) fn from_child(mut child: Child) -> Result<Self, HelperError> {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing helper stdout"))?;
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        let output = Arc::clone(&inbox);
        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                let result = reader
                    .by_ref()
                    .take((MAX_TELEMETRY_FRAME_BYTES + 1) as u64)
                    .read_until(b'\n', &mut line);
                let parsed = match result {
                    Ok(0) => break,
                    Ok(_) => decode_sample(&line),
                    Err(error) => Err(TelemetryError::Io(error)),
                };

                let Ok(mut inbox) = output.lock() else { break };
                match parsed {
                    Ok(sample) => inbox.latest = Some((sample, Instant::now())),
                    Err(error) => {
                        inbox.error = Some(Arc::new(error));
                        break;
                    }
                }
            }
        });
        let diagnostics = Diagnostics::new(child.stderr.take());

        Ok(Self {
            input: child.stdin.take(),
            child,
            reader: Some(reader),
            inbox,
            diagnostics,
            shutdown_timeout: SHUTDOWN_TIMEOUT,
        })
    }

    pub(crate) fn sample(&mut self) -> Result<Option<Sample>, HelperError> {
        if let Some(status) = self.child.try_wait()? {
            self.join_readers()?;
            return Err(self.diagnostics.attach(HelperError::Exited(status)));
        }

        let inbox = self.inbox.lock().map_err(|_| HelperError::TelemetryLock)?;
        if let Some(error) = &inbox.error {
            return Err(self
                .diagnostics
                .attach(HelperError::Telemetry(Arc::clone(error))));
        }
        let Some((sample, observed)) = &inbox.latest else {
            return Ok(None);
        };
        if observed.elapsed() > TELEMETRY_TIMEOUT {
            return Err(self.diagnostics.attach(HelperError::TelemetryStale));
        }

        Ok(Some(sample.clone()))
    }

    pub(crate) fn stop(&mut self) -> ShutdownReport {
        let process_result = self.stop_process();
        let exited = matches!(self.child.try_wait(), Ok(Some(_)));
        let (sample, telemetry_result) = match self.inbox.lock() {
            Ok(inbox) => {
                let sample = inbox.latest.as_ref().map(|(sample, _)| sample.clone());
                let result = match &inbox.error {
                    Some(error) => Err(HelperError::Telemetry(error.clone())),
                    None => Ok(()),
                };
                (sample, result)
            }
            Err(_) => (None, Err(HelperError::TelemetryLock)),
        };

        // A process failure takes precedence, while the last sample is always retained.
        let result = match process_result {
            Ok(()) => telemetry_result,
            Err(error) => Err(error),
        };

        ShutdownReport {
            sample,
            result: result.map_err(|error| self.diagnostics.attach(error)),
            exited,
        }
    }

    fn stop_process(&mut self) -> Result<(), HelperError> {
        if let Some(mut input) = self.input.take() {
            let _ = input.write_all(STOP_COMMAND);
        }

        let mut deadline = Instant::now() + self.shutdown_timeout;
        let mut forced = false;
        let status = loop {
            if let Some(status) = self.child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                if forced {
                    return Err(HelperError::TerminationTimeout);
                }
                self.child.kill()?;
                forced = true;
                deadline = Instant::now() + FORCED_TERMINATION_TIMEOUT;
            }
            thread::sleep(SHUTDOWN_POLL_INTERVAL);
        };

        self.join_readers()?;

        if forced {
            return Err(HelperError::ForcedTermination);
        }
        if !status.success() {
            return Err(HelperError::FailedExit(status));
        }

        Ok(())
    }

    fn join_readers(&mut self) -> Result<(), HelperError> {
        let telemetry = self
            .reader
            .take()
            .map(|reader| reader.join().map_err(|_| HelperError::TelemetryReader))
            .transpose();
        self.diagnostics.finish();
        telemetry.map(|_| ())
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        if self.reader.is_some() {
            let _ = self.stop();
        }
    }
}

#[cfg(test)]
mod tests;
