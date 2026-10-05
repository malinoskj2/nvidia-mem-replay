use super::*;

#[test]
fn latest_control_request_survives_a_full_wake_queue() {
    let (wake, receiver) = mpsc::sync_channel(1);
    let control = Control {
        pending: Arc::new(Mutex::new(Command::Idle)),
        wake,
        closing: Arc::new(AtomicBool::new(false)),
    };
    control.request(Command::Start(Config::default()));
    control.request(Command::Stop);
    assert!(receiver.try_recv().is_ok());
    assert!(matches!(*control.pending.lock().unwrap(), Command::Stop));
}

#[test]
fn shutdown_cannot_be_superseded_by_a_tray_or_window_action() {
    let (wake, _) = mpsc::sync_channel(1);
    let control = Control {
        pending: Arc::new(Mutex::new(Command::Idle)),
        wake,
        closing: Arc::new(AtomicBool::new(false)),
    };
    control.request(Command::Shutdown);
    control.request(Command::Start(Config::default()));
    assert!(matches!(
        *control.pending.lock().unwrap(),
        Command::Shutdown
    ));
}

fn sample(bytes: u64) -> Sample {
    Sample {
        version: 1,
        written_bytes: bytes,
        buffer_bytes: 0,
        resident_bytes: None,
        available_bytes: 1_000_000_000,
    }
}

fn redirect() -> Redirect {
    let original = crate::sys::nvidia::RawValue {
        kind: 1,
        bytes: Vec::new(),
    }
    .with_path(r"C:\NVIDIA");
    Redirect {
        replacement: original.with_path(r"R:\Temp"),
        original,
        original_path: r"C:\NVIDIA".to_owned(),
        target: r"R:\Temp".to_owned(),
    }
}

#[test]
fn failed_checkpoint_keeps_recording_and_retries_at_bounded_cadence() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    let now = Instant::now();
    let mut last_attempt = now;
    let mut status = Status {
        mounted: true,
        active: true,
        lifetime_bytes: 100,
        ..Status::default()
    };
    // A real filesystem failure at the storage boundary.
    std::fs::create_dir(directory.path().join("lifetime.pending")).unwrap();
    checkpoint(
        &store,
        &mut last_attempt,
        &mut status,
        now + Duration::from_secs(10),
    );
    assert!(status.mounted && status.active);
    assert_eq!(status.lifetime_bytes, 100);
    assert!(status.warning.is_some());
    assert!(status.error.is_none());
    assert!(status.lifetime_dirty);
    std::fs::remove_dir(directory.path().join("lifetime.pending")).unwrap();
    status.lifetime_bytes = 150;
    checkpoint(
        &store,
        &mut last_attempt,
        &mut status,
        now + Duration::from_secs(11),
    );
    assert_eq!(store.lifetime().unwrap(), 0);
    checkpoint(
        &store,
        &mut last_attempt,
        &mut status,
        now + Duration::from_secs(20),
    );
    assert_eq!(store.lifetime().unwrap(), 150);
    assert!(status.warning.is_none());
    assert!(!status.lifetime_dirty);
}

#[test]
fn final_accounting_survives_helper_failure_and_cleanup_errors_are_aggregated() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    store.save_redirect(&redirect()).unwrap();
    let mut meter = Meter::new(100);
    let mut status = Status {
        mounted: true,
        lifetime_bytes: 110,
        ..Status::default()
    };
    let error = finish_stop(
        &store,
        &mut meter,
        &mut status,
        Err(anyhow::anyhow!("registry unavailable")),
        ShutdownReport {
            exited: true,
            sample: Some(sample(25)),
            result: Err(crate::sys::helper::HelperError::ForcedTermination),
        },
    )
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("registry unavailable"));
    assert!(message.contains("forcibly terminated"));
    assert_eq!(status.lifetime_bytes, 125);
    assert_eq!(store.lifetime().unwrap(), 125);
    assert!(!status.mounted && !status.active);
    assert!(store.redirect().unwrap().is_some());
    recover_with(&store, |_| Ok(())).unwrap();
    assert!(store.redirect().unwrap().is_none());
}

#[test]
fn shutdown_reports_cleanup_failure_then_succeeds_on_retry() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    std::fs::create_dir(directory.path().join("lifetime.pending")).unwrap();
    let mut meter = Meter::new(0);
    let mut status = Status::default();
    let result = finish_stop(
        &store,
        &mut meter,
        &mut status,
        Ok(()),
        ShutdownReport {
            exited: true,
            sample: Some(sample(42)),
            result: Ok(()),
        },
    );
    complete_shutdown(&mut status, result);
    assert!(status.shutdown == Shutdown::Failed);
    assert!(status.error.as_ref().unwrap().contains("save lifetime"));
    assert_eq!(status.lifetime_bytes, 42);
    std::fs::remove_dir(directory.path().join("lifetime.pending")).unwrap();
    let result = stop(&store, &mut None, &mut status);
    complete_shutdown(&mut status, result);
    assert!(status.shutdown == Shutdown::Complete);
    assert!(status.error.is_none());
    assert_eq!(store.lifetime().unwrap(), 42);
}

#[test]
fn normal_commands_cannot_supersede_shutdown_after_worker_consumes_request() {
    let (wake, _) = mpsc::sync_channel(1);
    let control = Control {
        pending: Arc::new(Mutex::new(Command::Idle)),
        wake,
        closing: Arc::new(AtomicBool::new(false)),
    };
    control.request(Command::Shutdown);
    *control.pending.lock().unwrap() = Command::Idle;
    control.request(Command::Start(Config::default()));
    control.request(Command::Stop);
    assert!(matches!(*control.pending.lock().unwrap(), Command::Idle));
    control.request(Command::Shutdown);
    assert!(matches!(
        *control.pending.lock().unwrap(),
        Command::Shutdown
    ));
    control.request(Command::Exit);
    assert!(matches!(*control.pending.lock().unwrap(), Command::Exit));
}

#[test]
fn shutdown_retries_restoration_after_session_has_already_stopped() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    store.save_redirect(&redirect()).unwrap();
    let mut status = Status::default();
    let mut session = None;
    let failed = stop_with(&store, &mut session, &mut status, |_| {
        anyhow::bail!("registry unavailable")
    });
    complete_shutdown(&mut status, failed);
    assert!(status.shutdown == Shutdown::Failed);
    assert!(store.redirect().unwrap().is_some());
    let retried = stop_with(&store, &mut session, &mut status, |_| Ok(()));
    complete_shutdown(&mut status, retried);
    assert!(status.shutdown == Shutdown::Complete);
    assert!(status.error.is_none());
    assert!(store.redirect().unwrap().is_none());
    assert_eq!(status.message, "Stopped · temporary location restored");
}

#[test]
fn unconfirmed_helper_exit_retains_mounted_state_and_recovery_journal() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    store.save_redirect(&redirect()).unwrap();
    let mut meter = Meter::new(0);
    let mut status = Status::default();
    let result = finish_stop(
        &store,
        &mut meter,
        &mut status,
        Ok(()),
        ShutdownReport {
            sample: Some(sample(42)),
            exited: false,
            result: Err(crate::sys::helper::HelperError::TerminationTimeout),
        },
    );
    assert!(result.is_err());
    assert!(status.mounted);
    assert!(store.redirect().unwrap().is_some());
    assert_eq!(store.lifetime().unwrap(), 42);
}

#[cfg(unix)]
#[test]
fn poll_keeps_live_helper_and_session_when_lifetime_checkpoint_fails() {
    use std::process::{Command as ProcessCommand, Stdio};
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    std::fs::create_dir(directory.path().join("lifetime.pending")).unwrap();
    let child = ProcessCommand::new("sh")
        .args(["-c", r#"printf '%s\n' '{"version":1,"written_bytes":25,"buffer_bytes":0,"resident_bytes":null,"available_bytes":1000000000}'; read command"#])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut helper = Helper::from_child(child).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let first_sample = loop {
        if let Some(sample) = helper.sample().unwrap() {
            break sample;
        }
        assert!(
            Instant::now() < deadline,
            "helper did not publish telemetry"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    let now = Instant::now();
    let mut session = Session {
        helper,
        redirect: redirect(),
        meter: Meter::new(100),
        last_sample: now,
        checkpoint: now.checked_sub(Duration::from_secs(10)).unwrap(),
        stopping: false,
    };
    let mut status = Status {
        mounted: true,
        lifetime_bytes: session.meter.observe(&first_sample, now).unwrap(),
        ..Status::default()
    };
    assert!(poll(&store, &mut session, &mut status).is_ok());
    assert!(status.mounted);
    assert_eq!(status.lifetime_bytes, 125);
    assert!(status.warning.is_some());
    // The helper remains alive and can be stopped through the same owned session.
    assert!(session.helper.sample().is_ok());
    assert!(session.helper.stop().exited);
}
