use crate::{
    config::Config,
    filesystem::MAX_RECOVERY_SNAPSHOT_BYTES,
    sys::nvidia::Redirect,
    telemetry::{MAX_TELEMETRY_FRAME_BYTES, Sample, TELEMETRY_TIMEOUT, decode_sample},
};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use thiserror::Error;

const READY_TIMEOUT: Duration = Duration::from_secs(15);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const RECOVERY_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const FORCED_TERMINATION_TIMEOUT: Duration = Duration::from_secs(1);
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Error)]
pub(crate) enum HelperError {
    #[error("RAM filesystem helper I/O: {0}")]
    Io(#[from] io::Error),
    #[error("drive {0} is already in use; choose another drive letter")]
    Occupied(String),
    #[error(
        "RAM filesystem did not become ready within 15 seconds; run the installer to install WinFsp 2.1"
    )]
    Timeout,
    #[error(
        "RAM filesystem stopped unexpectedly; run the bundled installer to install or repair WinFsp 2.1, and restart Windows if requested"
    )]
    Exited,
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
    Telemetry(String),
}

#[derive(Default)]
struct Inbox {
    latest: Option<(Sample, Instant)>,
    error: Option<String>,
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
    shutdown_timeout: Duration,
}

impl Helper {
    pub(crate) fn start(config: &Config, redirect: &Redirect) -> Result<Self, HelperError> {
        if std::path::Path::new(&format!("{}\\", config.mount())).try_exists()? {
            return Err(HelperError::Occupied(config.mount()));
        }

        let mut command = Command::new(std::env::current_exe()?);
        command
            .args([
                "filesystem",
                "--drive",
                &config.drive.to_string(),
                "--memory-limit-mb",
                &config.memory_limit_mb.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }

        Self::wait_ready(command, config, Some(redirect))
    }

    #[cfg(windows)]
    pub(crate) fn start_memefs(config: &Config) -> Result<Self, HelperError> {
        use std::os::windows::process::CommandExt;
        let executable = std::env::current_exe()?.with_file_name("memefs-x64.exe");
        let mut command = Command::new(executable);
        command
            .args([
                "-m",
                &config.mount(),
                "-s",
                &config.limit_bytes().to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000);

        Self::wait_ready(command, config, None)
    }

    fn wait_ready(
        mut command: Command,
        config: &Config,
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
        if let Some(snapshot) = snapshot {
            // The first telemetry frame acknowledges receipt of this exact snapshot.
            helper.shutdown_timeout = RECOVERY_SHUTDOWN_TIMEOUT;
            let input = helper
                .input
                .as_mut()
                .ok_or_else(|| io::Error::other("missing helper stdin"))?;
            input.write_all(&snapshot)?;
            input.write_all(b"\n")?;
        }

        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if helper.sample()?.is_some() {
                std::fs::create_dir_all(config.target())?;
                return Ok(helper);
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
                    Err(error) => Err(error.to_string()),
                };

                let Ok(mut inbox) = output.lock() else { break };
                match parsed {
                    Ok(sample) => inbox.latest = Some((sample, Instant::now())),
                    Err(error) => {
                        inbox.error = Some(error);
                        break;
                    }
                }
            }
        });

        Ok(Self {
            input: child.stdin.take(),
            child,
            reader: Some(reader),
            inbox,
            shutdown_timeout: SHUTDOWN_TIMEOUT,
        })
    }

    pub(crate) fn sample(&mut self) -> Result<Option<Sample>, HelperError> {
        let inbox = self
            .inbox
            .lock()
            .map_err(|_| HelperError::Telemetry("telemetry lock failed".to_owned()))?;
        if let Some(error) = &inbox.error {
            return Err(HelperError::Telemetry(error.clone()));
        }
        if self.child.try_wait()?.is_some() {
            return Err(HelperError::Exited);
        }

        let Some((sample, observed)) = &inbox.latest else {
            return Ok(None);
        };
        if observed.elapsed() > TELEMETRY_TIMEOUT {
            return Err(HelperError::Telemetry(
                "helper stopped publishing telemetry".to_owned(),
            ));
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
            Err(_) => (
                None,
                Err(HelperError::Telemetry("telemetry lock failed".to_owned())),
            ),
        };

        // A process failure takes precedence, while the last sample is always retained.
        let result = match process_result {
            Ok(()) => telemetry_result,
            Err(error) => Err(error),
        };

        ShutdownReport {
            sample,
            result,
            exited,
        }
    }

    fn stop_process(&mut self) -> Result<(), HelperError> {
        if let Some(mut input) = self.input.take() {
            let _ = input.write_all(b"stop\n");
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

        if let Some(reader) = self.reader.take() {
            reader
                .join()
                .map_err(|_| HelperError::Telemetry("reader thread failed".to_owned()))?;
        }

        if forced {
            return Err(HelperError::ForcedTermination);
        }
        if !status.success() {
            return Err(HelperError::FailedExit(status));
        }

        Ok(())
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        if self.reader.is_some() {
            let _ = self.stop();
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
