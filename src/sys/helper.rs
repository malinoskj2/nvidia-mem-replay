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
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
            command.creation_flags(CREATE_NO_WINDOW);
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
            .creation_flags(CREATE_NO_WINDOW);

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
mod tests {
    use super::*;

    #[cfg(unix)]
    const SAMPLE: &str = r#"{"version":1,"written_bytes":17,"buffer_bytes":0,"resident_bytes":0,"available_bytes":200}"#;

    #[cfg(unix)]
    fn helper(script: &str) -> Helper {
        let child = Command::new("sh")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        Helper::from_child(child).unwrap()
    }

    #[cfg(unix)]
    fn wait_for_sample(helper: &mut Helper) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while helper.sample().unwrap().is_none() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_missing_mount_point_is_free() {
        let directory = tempfile::tempdir().unwrap();
        let mount = directory.path().join("ram").display().to_string();

        ensure_mount_point_free(&mount).unwrap();

        assert!(!directory.path().join("ram").exists());
    }

    #[test]
    fn an_existing_directory_occupies_the_mount_point() {
        let directory = tempfile::tempdir().unwrap();
        let mount = directory.path().join("ram");
        std::fs::create_dir(&mount).unwrap();
        let mount = mount.display().to_string();

        let error = ensure_mount_point_free(&mount).unwrap_err();

        assert!(matches!(error, HelperError::Occupied(ref occupied) if *occupied == mount));
        assert!(error.to_string().contains("already in use"));
        assert!(directory.path().join("ram").is_dir());
    }

    /// A junction needs no privilege, unlike a symbolic link.
    #[cfg(windows)]
    #[test]
    fn a_dangling_junction_is_removed_and_a_live_one_occupies_the_mount_point() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let junction = directory.path().join("ram");
        let created = Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(created.success());
        let mount = junction.display().to_string();

        assert!(matches!(
            ensure_mount_point_free(&mount),
            Err(HelperError::Occupied(_))
        ));

        std::fs::remove_dir(&target).unwrap();
        assert!(std::fs::symlink_metadata(&junction).is_ok());

        ensure_mount_point_free(&mount).unwrap();

        assert!(std::fs::symlink_metadata(&junction).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_joins_reader_and_keeps_the_final_counter() {
        let mut helper = helper(&format!(
            "printf '%s\\n' '{}'; read command; printf '%s\\n' '{SAMPLE}'",
            SAMPLE.replace("17", "5")
        ));
        wait_for_sample(&mut helper);

        let stopped = helper.stop();

        stopped.result.unwrap();
        assert!(stopped.exited);
        assert_eq!(stopped.sample.unwrap().written_bytes, 17);
        assert!(helper.reader.is_none());
        assert!(helper.child.try_wait().unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn nonzero_exit_keeps_final_sample_and_reports_status() {
        let mut helper = helper(&format!("read command; printf '%s\\n' '{SAMPLE}'; exit 42"));

        let stopped = helper.stop();

        assert!(stopped.exited);
        assert_eq!(stopped.sample.unwrap().written_bytes, 17);
        match stopped.result.unwrap_err() {
            HelperError::FailedExit(status) => assert_eq!(status.code(), Some(42)),
            error => panic!("unexpected shutdown error: {error}"),
        }
        assert!(helper.reader.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn malformed_final_frame_keeps_last_sample_and_reports_failure() {
        let mut helper = helper(&format!(
            "read command; printf '%s\\n' '{SAMPLE}'; printf '%s\\n' 'invalid JSON'"
        ));

        let stopped = helper.stop();

        assert!(stopped.exited);
        assert_eq!(stopped.sample.unwrap().written_bytes, 17);
        assert!(matches!(
            stopped.result,
            Err(HelperError::Telemetry(error)) if matches!(*error, TelemetryError::Json(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn unsupported_final_protocol_keeps_last_valid_sample() {
        let mut helper = helper(&format!(
            "read command; printf '%s\\n' '{SAMPLE}'; printf '%s\\n' '{}'",
            SAMPLE.replace("\"version\":1", "\"version\":2")
        ));

        let stopped = helper.stop();

        assert_eq!(stopped.sample.unwrap().version, 1);
        assert!(matches!(
            stopped.result,
            Err(HelperError::Telemetry(error)) if matches!(*error, TelemetryError::Version(2))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn forced_shutdown_is_reported_and_keeps_the_latest_sample() {
        let mut helper = helper(&format!(
            "printf '%s\\n' '{SAMPLE}'; read command; exec sleep 30"
        ));
        wait_for_sample(&mut helper);
        helper.shutdown_timeout = Duration::from_millis(100);

        let stopped = helper.stop();

        assert!(stopped.exited);
        assert!(matches!(
            stopped.result,
            Err(HelperError::ForcedTermination)
        ));
        assert_eq!(stopped.sample.unwrap().written_bytes, 17);
        assert!(helper.reader.is_none());
        assert!(helper.child.try_wait().unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn oversized_output_is_bounded_and_rejected() {
        let mut helper = helper("printf '%2000s' x; read command");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if matches!(helper.sample(), Err(HelperError::Telemetry(_))) {
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(
            helper.stop().result,
            Err(HelperError::Telemetry(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn process_failure_takes_precedence_over_invalid_final_telemetry() {
        let mut helper = helper(&format!(
            "read command; printf '%s\\n' '{SAMPLE}'; printf '%s\\n' 'invalid JSON'; exit 42"
        ));

        let stopped = helper.stop();

        assert_eq!(stopped.sample.unwrap().written_bytes, 17);
        assert!(matches!(stopped.result, Err(HelperError::FailedExit(_))));
    }

    #[cfg(unix)]
    #[test]
    fn unexpected_exit_preserves_stderr_and_exit_status() {
        let mut helper = helper("printf '%s\\n' 'registry access denied' >&2; exit 42");
        let deadline = Instant::now() + Duration::from_secs(2);
        let error = loop {
            if let Err(error) = helper.sample() {
                break error;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        };

        assert!(error.to_string().contains("registry access denied"));
        match error {
            HelperError::Diagnostics { error, .. } => {
                assert!(matches!(*error, HelperError::Exited(status) if status.code() == Some(42)));
            }
            error => panic!("unexpected helper error: {error}"),
        }
        assert!(helper.stop().exited);
    }

    #[cfg(unix)]
    #[test]
    fn readiness_failure_preserves_helper_diagnostics() {
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "printf '%s\\n' 'mount permission denied' >&2; exit 42",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let error = Helper::wait_ready(command, &Volume::new("T:".to_owned(), 256), None)
            .err()
            .unwrap();

        assert!(error.to_string().contains("mount permission denied"));
        match error {
            HelperError::Diagnostics { error, .. } => {
                assert!(matches!(*error, HelperError::Exited(status) if status.code() == Some(42)));
            }
            error => panic!("unexpected readiness error: {error}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn failed_shutdown_preserves_final_sample_and_stderr() {
        let mut helper = helper(&format!(
            "read command; printf '%s\\n' '{SAMPLE}'; printf '%s\\n' 'filesystem stop failed' >&2; exit 42"
        ));

        let stopped = helper.stop();

        assert!(stopped.exited);
        assert_eq!(stopped.sample.unwrap().written_bytes, 17);
        let error = stopped.result.unwrap_err();
        assert!(error.to_string().contains("filesystem stop failed"));
        match error {
            HelperError::Diagnostics { error, .. } => {
                assert!(
                    matches!(*error, HelperError::FailedExit(status) if status.code() == Some(42))
                );
            }
            error => panic!("unexpected shutdown error: {error}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn excess_stderr_is_capped_and_drained_without_blocking_the_helper() {
        let mut helper = helper(&format!(
            "head -c 100000 /dev/zero | tr '\\000' x >&2; printf '%s\\n' '{SAMPLE}'; read command; exit 42"
        ));
        wait_for_sample(&mut helper);

        let stopped = helper.stop();

        assert!(stopped.exited);
        assert_eq!(stopped.sample.unwrap().written_bytes, 17);
        match stopped.result.unwrap_err() {
            HelperError::Diagnostics { error, diagnostics } => {
                assert!(
                    matches!(*error, HelperError::FailedExit(status) if status.code() == Some(42))
                );
                assert_eq!(
                    diagnostics.trim().lines().next().unwrap().len(),
                    diagnostics::MAX_DIAGNOSTIC_BYTES
                );
                assert!(diagnostics.contains("stderr truncated"));
            }
            error => panic!("unexpected shutdown error: {error}"),
        }
    }
}
