use crate::{config::Config, sys::nvidia::Redirect, telemetry::Sample};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use thiserror::Error;

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
        if snapshot.as_ref().is_some_and(|bytes| bytes.len() >= 65_536) {
            return Err(HelperError::SnapshotOversized);
        }
        let mut helper = Self::from_child(command.spawn()?)?;
        if let Some(snapshot) = snapshot {
            // The first telemetry frame acknowledges receipt of this exact snapshot.
            helper.shutdown_timeout = Duration::from_secs(10);
            let input = helper
                .input
                .as_mut()
                .ok_or_else(|| io::Error::other("missing helper stdin"))?;
            input.write_all(&snapshot)?;
            input.write_all(b"\n")?;
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(sample) = helper.sample()? {
                if sample.version != 1 {
                    return Err(HelperError::Telemetry("unsupported protocol".to_owned()));
                }
                std::fs::create_dir_all(config.target())?;
                return Ok(helper);
            }
            if Instant::now() >= deadline {
                return Err(HelperError::Timeout);
            }
            thread::sleep(Duration::from_millis(100));
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
                let result = reader.by_ref().take(1025).read_until(b'\n', &mut line);
                let parsed = match result {
                    Ok(0) => break,
                    Ok(_) if line.len() > 1024 || !line.ends_with(b"\n") => {
                        Err("invalid or oversized helper telemetry".to_owned())
                    }
                    Ok(_) => serde_json::from_slice::<Sample>(&line)
                        .map_err(|error| error.to_string())
                        .and_then(|sample| {
                            if sample.version == 1 {
                                Ok(sample)
                            } else {
                                Err(format!("unsupported protocol version {}", sample.version))
                            }
                        }),
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
            shutdown_timeout: Duration::from_secs(5),
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
        match &inbox.latest {
            Some((_, observed)) if observed.elapsed() > Duration::from_secs(3) => Err(
                HelperError::Telemetry("helper stopped publishing telemetry".to_owned()),
            ),
            sample => Ok(sample.as_ref().map(|(sample, _)| sample.clone())),
        }
    }

    pub(crate) fn stop(&mut self) -> ShutdownReport {
        let result = self.stop_process();
        let exited = self.child.try_wait().is_ok_and(|status| status.is_some());
        let inbox = self.inbox.lock();
        match inbox {
            Ok(inbox) => ShutdownReport {
                exited,
                sample: inbox.latest.as_ref().map(|(sample, _)| sample.clone()),
                result: result.and_then(|()| {
                    inbox
                        .error
                        .as_ref()
                        .map_or(Ok(()), |error| Err(HelperError::Telemetry(error.clone())))
                }),
            },
            Err(_) => ShutdownReport {
                exited,
                sample: None,
                result: result
                    .and_then(|()| Err(HelperError::Telemetry("telemetry lock failed".to_owned()))),
            },
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
                deadline = Instant::now() + Duration::from_secs(1);
            }
            thread::sleep(Duration::from_millis(50));
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
        if self.reader.is_some()
            || self
                .child
                .try_wait()
                .map_or(true, |status| status.is_none())
        {
            let _ = self.stop();
        }
    }
}

#[cfg(all(test, unix))]
#[path = "../../tests/unit/helper.rs"]
mod tests;
