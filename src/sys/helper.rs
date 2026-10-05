use crate::{config::Config, telemetry::Sample};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
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
    #[error("helper telemetry: {0}")]
    Telemetry(String),
}

#[derive(Default)]
struct Inbox {
    latest: Option<(Sample, Instant)>,
    error: Option<String>,
}

pub(crate) struct Helper {
    child: Child,
    input: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
    inbox: Arc<Mutex<Inbox>>,
}

impl Helper {
    pub(crate) fn start(config: &Config) -> Result<Self, HelperError> {
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
        Self::wait_ready(command, config)
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
        Self::wait_ready(command, config)
    }

    fn wait_ready(mut command: Command, config: &Config) -> Result<Self, HelperError> {
        let mut helper = Self::from_child(command.spawn()?)?;
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

    fn from_child(mut child: Child) -> Result<Self, HelperError> {
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
                    Ok(_) => {
                        serde_json::from_slice::<Sample>(&line).map_err(|error| error.to_string())
                    }
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

    pub(crate) fn stop(&mut self) -> Result<Option<Sample>, HelperError> {
        if let Some(mut input) = self.input.take() {
            let _ = input.write_all(b"stop\n");
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.child.try_wait()?.is_none() {
            if Instant::now() >= deadline {
                self.child.kill()?;
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        self.child.wait()?;
        if let Some(reader) = self.reader.take() {
            reader
                .join()
                .map_err(|_| HelperError::Telemetry("reader thread failed".to_owned()))?;
        }
        let inbox = self
            .inbox
            .lock()
            .map_err(|_| HelperError::Telemetry("telemetry lock failed".to_owned()))?;
        Ok(inbox.latest.as_ref().map(|(sample, _)| sample.clone()))
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
mod tests {
    use super::*;

    #[test]
    fn shutdown_joins_reader_and_keeps_the_final_counter() {
        let child = Command::new("sh")
            .args(["-c", "printf '%s\\n' '{\"version\":1,\"written_bytes\":5,\"buffer_bytes\":100,\"resident_bytes\":50,\"available_bytes\":200}'; read command; printf '%s\\n' '{\"version\":1,\"written_bytes\":17,\"buffer_bytes\":0,\"resident_bytes\":0,\"available_bytes\":200}'"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut helper = Helper::from_child(child).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while helper.sample().unwrap().is_none() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(helper.stop().unwrap().unwrap().written_bytes, 17);
        assert!(helper.reader.is_none());
        assert!(helper.child.try_wait().unwrap().is_some());
    }

    #[test]
    fn oversized_output_is_bounded_and_rejected() {
        let child = Command::new("sh")
            .args(["-c", "printf '%2000s' x; read command"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut helper = Helper::from_child(child).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if matches!(helper.sample(), Err(HelperError::Telemetry(_))) {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        helper.stop().unwrap();
    }
}
