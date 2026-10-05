use super::*;
use crate::sys::helper::ShutdownReport;
use cleanup::{finish_stop, stop_with};

fn control() -> (Control, Receiver<()>) {
    let (wake, receiver) = mpsc::sync_channel(1);
    (
        Control {
            mailbox: Arc::new(Mutex::new(Mailbox::default())),
            wake,
        },
        receiver,
    )
}

#[test]
fn latest_control_request_survives_a_full_wake_queue() {
    let (control, receiver) = control();
    control.request(Command::Start(Config::default()));
    control.request(Command::Stop);
    assert!(receiver.try_recv().is_ok());
    assert!(matches!(
        control.mailbox.lock().unwrap().pending,
        Some(Command::Stop)
    ));
}

#[test]
fn shutdown_cannot_be_superseded_by_a_tray_or_window_action() {
    let (control, _) = control();
    control.request(Command::Shutdown);
    control.request(Command::Start(Config::default()));
    assert!(matches!(
        control.mailbox.lock().unwrap().pending,
        Some(Command::Shutdown)
    ));
}

#[test]
fn normal_commands_cannot_supersede_shutdown_after_worker_consumes_request() {
    let (control, _) = control();
    control.request(Command::Shutdown);
    control.mailbox.lock().unwrap().pending.take();
    control.request(Command::Start(Config::default()));
    control.request(Command::Stop);
    assert!(control.mailbox.lock().unwrap().pending.is_none());
    control.request(Command::Shutdown);
    assert!(matches!(
        control.mailbox.lock().unwrap().pending,
        Some(Command::Shutdown)
    ));
    control.request(Command::Exit);
    control.request(Command::Shutdown);
    assert!(matches!(
        control.mailbox.lock().unwrap().pending,
        Some(Command::Exit)
    ));
}

fn sample(bytes: u64) -> Sample {
    Sample {
        version: crate::telemetry::TELEMETRY_PROTOCOL_VERSION,
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
    let mut accounting = Accounting {
        total: 100,
        dirty: true,
        ..Accounting::default()
    };
    std::fs::create_dir(directory.path().join("lifetime.pending")).unwrap();
    accounting.checkpoint(&store, &mut last_attempt, now + Duration::from_secs(10));
    assert_eq!(accounting.total, 100);
    assert!(accounting.warning.is_some());
    assert!(accounting.dirty);
    std::fs::remove_dir(directory.path().join("lifetime.pending")).unwrap();
    accounting.total = 150;
    accounting.checkpoint(&store, &mut last_attempt, now + Duration::from_secs(11));
    assert_eq!(store.lifetime().unwrap(), 0);
    accounting.checkpoint(&store, &mut last_attempt, now + Duration::from_secs(20));
    assert_eq!(store.lifetime().unwrap(), 150);
    assert!(accounting.warning.is_none());
    assert!(!accounting.dirty);
}

#[test]
fn final_accounting_survives_helper_failure_and_cleanup_errors_keep_sources() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    store.save_redirect(&redirect()).unwrap();
    let mut meter = Meter::new(100);
    let mut state = State::default();
    let report = finish_stop(
        &store,
        &mut meter,
        &mut state,
        Err(anyhow::anyhow!("registry unavailable")),
        ShutdownReport {
            exited: true,
            sample: Some(sample(25)),
            result: Err(crate::sys::helper::HelperError::ForcedTermination),
        },
    );
    assert!(!report.is_ok());
    assert!(matches!(
        report
            .helper
            .as_ref()
            .unwrap_err()
            .downcast_ref::<crate::sys::helper::HelperError>(),
        Some(crate::sys::helper::HelperError::ForcedTermination)
    ));
    assert!(report.accounting.is_ok());
    assert!(report.persistence.is_ok());
    let message = report.to_string();
    assert!(message.contains("registry unavailable"));
    assert!(message.contains("forcibly terminated"));
    assert_eq!(state.accounting.total, 125);
    assert_eq!(store.lifetime().unwrap(), 125);
    assert_eq!(state.message, DisplayStatus::CleanupFailed);
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
    let mut state = State::default();
    let report = finish_stop(
        &store,
        &mut meter,
        &mut state,
        Ok(()),
        ShutdownReport {
            exited: true,
            sample: Some(sample(42)),
            result: Ok(()),
        },
    );
    complete_shutdown(&mut state, &report);
    assert!(state.shutdown == Shutdown::Failed);
    assert!(state.error.as_ref().unwrap().contains("save lifetime"));
    assert_eq!(state.accounting.total, 42);
    std::fs::remove_dir(directory.path().join("lifetime.pending")).unwrap();
    let report = stop(&store, &mut state);
    complete_shutdown(&mut state, &report);
    assert!(state.shutdown == Shutdown::Complete);
    assert!(state.error.is_none());
    assert_eq!(store.lifetime().unwrap(), 42);
}

#[test]
fn shutdown_retries_restoration_after_session_has_already_stopped() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    store.save_redirect(&redirect()).unwrap();
    let mut state = State::default();
    let failed = stop_with(&store, &mut state, |_| {
        anyhow::bail!("registry unavailable")
    });
    complete_shutdown(&mut state, &failed);
    assert!(state.shutdown == Shutdown::Failed);
    assert!(store.redirect().unwrap().is_some());
    let retried = stop_with(&store, &mut state, |_| Ok(()));
    complete_shutdown(&mut state, &retried);
    assert!(state.shutdown == Shutdown::Complete);
    assert!(state.error.is_none());
    assert!(store.redirect().unwrap().is_none());
    assert_eq!(state.message, DisplayStatus::Stopped);
}

#[test]
fn unconfirmed_helper_exit_keeps_recovery_journal() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    store.save_redirect(&redirect()).unwrap();
    let mut meter = Meter::new(0);
    let mut state = State::default();
    let report = finish_stop(
        &store,
        &mut meter,
        &mut state,
        Ok(()),
        ShutdownReport {
            sample: Some(sample(42)),
            exited: false,
            result: Err(crate::sys::helper::HelperError::TerminationTimeout),
        },
    );
    assert!(!report.is_ok());
    assert_eq!(state.message, DisplayStatus::HelperRunning);
    assert!(store.redirect().unwrap().is_some());
    assert_eq!(store.lifetime().unwrap(), 42);
}

#[test]
fn unsaved_accounting_survives_reinitialization() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    store.save_lifetime(5).unwrap();
    let mut state = State::default();
    state.accounting.total = 42;
    state.accounting.dirty = true;
    initialize(&store, &mut state).unwrap();
    assert_eq!(state.snapshot().lifetime_bytes, 42);
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
        sample: None,
        last_sample: now,
        checkpoint: now.checked_sub(Duration::from_secs(10)).unwrap(),
        stopping: false,
    };
    let mut state = State::default();
    state
        .accounting
        .observe(&mut session.meter, &first_sample, now)
        .unwrap();
    state.session = Some(session);
    assert!(
        poll(
            &store,
            state.session.as_mut().unwrap(),
            &mut state.accounting,
            &mut state.message
        )
        .is_ok()
    );
    let snapshot = state.snapshot();
    assert!(snapshot.mounted);
    assert_eq!(snapshot.lifetime_bytes, 125);
    assert!(snapshot.warning.is_some());
    assert!(snapshot.error.is_none());
    // The helper remains alive and can be stopped through the same owned session.
    let running = state.session.as_mut().unwrap();
    assert!(running.helper.sample().is_ok());
    assert!(running.helper.stop().exited);
}
