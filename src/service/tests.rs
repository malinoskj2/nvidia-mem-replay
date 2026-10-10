use super::*;
use crate::sys::helper::ShutdownReport;
use cleanup::{finish_stop, stop_with};
use std::process::{Command as ProcessCommand, Stdio};

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

/// A helper process that publishes nothing and exits on the stop command, so a `Session` can
/// exist without a filesystem.
fn idle_helper() -> Helper {
    #[cfg(windows)]
    let mut command = {
        let mut command = ProcessCommand::new("cmd");
        command.args(["/d", "/c", "set", "/p", "line="]);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = ProcessCommand::new("sh");
        command.args(["-c", "read command"]);
        command
    };
    let child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    Helper::from_child(child).unwrap()
}

fn idle_session() -> Session {
    Session {
        helper: idle_helper(),
        redirect: redirect(),
        meter: Meter::new(0),
        sample: None,
        checkpoint: Instant::now(),
        stopping: false,
    }
}

/// The error `nvidia::plan` reports while the NVIDIA App is not running.
fn unreachable_engine() -> anyhow::Error {
    #[cfg(windows)]
    let error = nvidia::NvidiaError::Unreachable(crate::sys::shadowplay::ApiError::Create(-1));
    #[cfg(not(windows))]
    let error = nvidia::NvidiaError::Unsupported;

    anyhow::Error::from(error).context("discover NVIDIA temporary files location")
}

fn past(seconds: u64) -> Instant {
    Instant::now()
        .checked_sub(Duration::from_secs(seconds))
        .unwrap()
}

#[test]
fn unreachable_engine_waits_for_nvidia_and_schedules_a_retry() {
    let mut state = State {
        error: Some("stale".to_owned()),
        ..State::default()
    };

    record_start_failure(&mut state, &unreachable_engine());

    assert_eq!(state.message, DisplayStatus::WaitingForNvidia);
    assert!(state.error.is_none());
    let retry = state.retry.unwrap();
    assert!(retry > Instant::now());
    assert!(retry <= Instant::now() + NVIDIA_RETRY_INTERVAL);
    let snapshot = state.snapshot();
    assert_eq!(snapshot.message, DisplayStatus::WaitingForNvidia);
    assert!(snapshot.error.is_none());
}

#[test]
fn other_start_failures_are_reported_without_a_retry() {
    let refused =
        anyhow::Error::from(nvidia::NvidiaError::Path).context("redirect NVIDIA temporary files");
    for error in [refused, anyhow::anyhow!("mount RAM filesystem: busy")] {
        let mut state = State {
            message: DisplayStatus::WaitingForNvidia,
            retry: Some(Instant::now()),
            ..State::default()
        };

        record_start_failure(&mut state, &error);

        assert_eq!(state.message, DisplayStatus::StartFailed);
        assert!(state.retry.is_none());
        let text = state.error.as_deref().unwrap();
        assert!(text.contains(&format!("{error:#}")), "{text}");
    }
}

#[test]
fn retry_waits_for_its_time_and_for_the_session_to_end() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    let mut attempts = 0;
    let mut attempt = |_: &Store, _: &Config, _: &mut State| -> Result<()> {
        attempts += 1;
        Ok(())
    };

    // No retry scheduled at all.
    let mut state = State::default();
    retry_start_with(&store, &mut state, &mut attempt);

    // Scheduled, but not yet due.
    let due_later = Instant::now() + NVIDIA_RETRY_INTERVAL;
    state.retry = Some(due_later);
    retry_start_with(&store, &mut state, &mut attempt);
    assert_eq!(state.retry, Some(due_later));

    // Due, but a session already exists.
    let due = past(1);
    state.retry = Some(due);
    state.session = Some(idle_session());
    retry_start_with(&store, &mut state, &mut attempt);
    assert_eq!(state.retry, Some(due));

    assert_eq!(attempts, 0);
}

#[test]
fn a_due_retry_attempts_a_start_and_records_its_outcome() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    store.save_lifetime(7).unwrap();
    let config = Config {
        memory_limit_mb: 1024,
    };
    let mut state = State {
        message: DisplayStatus::WaitingForNvidia,
        config: config.clone(),
        retry: Some(past(1)),
        ..State::default()
    };

    let mut attempted = None;
    retry_start_with(&store, &mut state, |_, config, _| {
        attempted = Some(config.memory_limit_mb);
        Err(unreachable_engine())
    });

    assert_eq!(attempted, Some(1024));
    assert_eq!(state.message, DisplayStatus::WaitingForNvidia);
    assert!(state.retry.is_some_and(|retry| retry > Instant::now()));
    assert_eq!(state.accounting.total, 7);

    state.retry = Some(past(1));
    retry_start_with(&store, &mut state, |_, _, _| {
        anyhow::bail!("mount RAM filesystem: busy")
    });

    assert_eq!(state.message, DisplayStatus::StartFailed);
    assert!(state.retry.is_none());
    assert!(state.error.as_deref().unwrap().contains("busy"));

    state.retry = Some(past(1));
    retry_start_with(&store, &mut state, |_, _, state| {
        state.message = DisplayStatus::Waiting;
        Ok(())
    });

    assert_eq!(state.message, DisplayStatus::Waiting);
    assert!(state.retry.is_none());
    assert!(state.error.is_none());
}

#[test]
fn redirection_check_is_skipped_while_stopping_or_within_the_interval() {
    let mut asked = 0;
    let mut ask = |_: &Redirect| -> Result<bool, nvidia::NvidiaError> {
        asked += 1;
        Ok(false)
    };
    let mut applied = 0;
    let mut apply = |_: &Redirect| -> Result<(), nvidia::NvidiaError> {
        applied += 1;
        Ok(())
    };

    // No session.
    let mut state = State::default();
    keep_redirected_with(&mut state, &mut (), &mut ask, &mut apply);
    assert!(state.redirection_checked.is_none());

    // Checked less than five seconds ago.
    state.session = Some(idle_session());
    let recent = past(1);
    state.redirection_checked = Some(recent);
    keep_redirected_with(&mut state, &mut (), &mut ask, &mut apply);
    assert_eq!(state.redirection_checked, Some(recent));

    // Due, but the session is on its way out.
    let long_ago = past(6);
    state.redirection_checked = Some(long_ago);
    state.session.as_mut().unwrap().stopping = true;
    keep_redirected_with(&mut state, &mut (), &mut ask, &mut apply);
    assert_eq!(state.redirection_checked, Some(long_ago));

    assert_eq!(asked, 0);
    assert_eq!(applied, 0);
    assert!(state.notice.is_none());
}

#[test]
fn a_location_that_still_points_at_ram_or_cannot_be_read_is_left_alone() {
    for answer in [Ok(true), Err(nvidia::NvidiaError::Path)] {
        let mut state = State {
            session: Some(idle_session()),
            redirection_checked: Some(past(6)),
            ..State::default()
        };
        let mut applied = false;

        keep_redirected_with(
            &mut state,
            &mut (),
            |_| answer,
            |_| {
                applied = true;
                Ok(())
            },
        );

        assert!(!applied);
        assert!(state.notice.is_none());
        assert!(
            state
                .redirection_checked
                .is_some_and(|checked| checked.elapsed() < Duration::from_secs(5))
        );
    }
}

#[test]
fn a_changed_location_is_redirected_again_and_reported() {
    let mut state = State {
        session: Some(idle_session()),
        redirection_checked: None,
        ..State::default()
    };
    let mut applied_to = None;

    keep_redirected_with(
        &mut state,
        &mut (),
        |_| Ok(false),
        |redirect| {
            applied_to = Some(redirect.target.clone());
            Ok(())
        },
    );

    assert_eq!(applied_to.as_deref(), Some(r"R:\Temp"));
    assert_eq!(state.notice.as_deref(), Some(text::REDIRECTED_AGAIN));
    assert!(state.redirection_checked.is_some());

    state.redirection_checked = Some(past(6));
    keep_redirected_with(
        &mut state,
        &mut (),
        |_| Ok(false),
        |_| Err(nvidia::NvidiaError::Path),
    );

    let notice = state.notice.as_deref().unwrap();
    assert!(notice.contains("could not be redirected again"), "{notice}");
    assert!(
        notice.contains(&nvidia::NvidiaError::Path.to_string()),
        "{notice}"
    );
}

#[test]
fn failed_checkpoint_keeps_recording_and_retries_at_bounded_cadence() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();

    let now = Instant::now();
    let mut last_attempt = now;
    let mut accounting = Accounting::default();
    let mut meter = Meter::new(0);
    accounting.observe(&mut meter, &sample(100), now).unwrap();
    std::fs::create_dir(directory.path().join("lifetime.pending")).unwrap();

    accounting.checkpoint(&store, &mut last_attempt, now + Duration::from_secs(10));

    assert_eq!(accounting.total, 100);
    assert!(accounting.warning.is_some());
    assert!(accounting.dirty);

    accounting
        .observe(&mut meter, &sample(100), now + Duration::from_secs(10))
        .unwrap();
    assert!(accounting.dirty);

    std::fs::remove_dir(directory.path().join("lifetime.pending")).unwrap();
    accounting
        .observe(&mut meter, &sample(150), now + Duration::from_secs(11))
        .unwrap();
    accounting.checkpoint(&store, &mut last_attempt, now + Duration::from_secs(11));

    assert_eq!(store.lifetime().unwrap(), 0);

    accounting.checkpoint(&store, &mut last_attempt, now + Duration::from_secs(20));

    assert_eq!(store.lifetime().unwrap(), 150);
    assert!(accounting.warning.is_none());
    assert!(!accounting.dirty);
}

#[test]
fn unchanged_telemetry_does_not_rewrite_lifetime_counter() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    store.save_lifetime(100).unwrap();
    let mut accounting = Accounting::default();
    accounting.load(&store).unwrap();
    let mut meter = Meter::new(accounting.total);
    let now = Instant::now();
    let mut last_attempt = now;

    // A write would fail; an unchanged counter should never attempt one.
    std::fs::create_dir(directory.path().join("lifetime.pending")).unwrap();
    accounting.observe(&mut meter, &sample(0), now).unwrap();
    accounting.checkpoint(&store, &mut last_attempt, now + Duration::from_secs(10));
    assert!(accounting.warning.is_none());
    assert_eq!(store.lifetime().unwrap(), 100);

    std::fs::remove_dir(directory.path().join("lifetime.pending")).unwrap();
    accounting
        .observe(&mut meter, &sample(25), now + Duration::from_secs(11))
        .unwrap();
    accounting.checkpoint(&store, &mut last_attempt, now + Duration::from_secs(20));
    assert_eq!(store.lifetime().unwrap(), 125);

    std::fs::create_dir(directory.path().join("lifetime.pending")).unwrap();
    accounting
        .observe(&mut meter, &sample(25), now + Duration::from_secs(21))
        .unwrap();
    accounting.checkpoint(&store, &mut last_attempt, now + Duration::from_secs(30));
    assert!(accounting.warning.is_none());
    assert_eq!(store.lifetime().unwrap(), 125);
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

    let failed = stop_with(&store, &mut state, &mut (), |_| {
        anyhow::bail!("registry unavailable")
    });
    complete_shutdown(&mut state, &failed);

    assert!(state.shutdown == Shutdown::Failed);
    assert!(store.redirect().unwrap().is_some());

    let retried = stop_with(&store, &mut state, &mut (), |_| Ok(()));
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
fn unsaved_accounting_survives_reload() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    store.save_lifetime(5).unwrap();
    let mut state = State::default();
    state.accounting.total = 42;
    state.accounting.dirty = true;

    state.accounting.load(&store).unwrap();

    assert_eq!(state.snapshot().lifetime_bytes, 42);
}

#[cfg(unix)]
#[test]
fn poll_keeps_live_helper_and_session_when_lifetime_checkpoint_fails() {
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

#[test]
fn invalid_restart_publishes_error_without_stopping_or_polling() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    let mut state = State {
        message: DisplayStatus::Ready,
        ..State::default()
    };
    state.accounting.total = 42;
    state.accounting.dirty = true;
    let output = Mutex::new(Status::default());
    let config = Config { memory_limit_mb: 0 };

    let outcome = handle_command(&store, Some(Command::Start(config)), &mut state, &output);

    assert!(matches!(outcome, CommandOutcome::Continue));
    assert_eq!(state.message, DisplayStatus::Ready);
    assert!(state.accounting.dirty);
    assert_eq!(store.lifetime().unwrap(), 0);
    let published = output.lock().unwrap();
    assert_eq!(published.message, DisplayStatus::Ready);
    assert_eq!(published.lifetime_bytes, 42);
    assert_eq!(published.error, state.error);
    assert!(published.error.as_ref().unwrap().contains("memory ceiling"));
}

#[test]
fn shutdown_command_stays_running_after_cleanup_failure_and_exits_on_retry() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    std::fs::create_dir(directory.path().join("lifetime.pending")).unwrap();
    let mut state = State::default();
    state.accounting.total = 42;
    state.accounting.dirty = true;
    let output = Mutex::new(Status::default());

    let outcome = handle_command(&store, Some(Command::Shutdown), &mut state, &output);

    assert!(matches!(outcome, CommandOutcome::Poll));
    assert!(output.lock().unwrap().shutdown == Shutdown::Failed);
    assert!(state.error.as_ref().unwrap().contains("save lifetime"));

    std::fs::remove_dir(directory.path().join("lifetime.pending")).unwrap();

    let outcome = handle_command(&store, Some(Command::Shutdown), &mut state, &output);

    assert!(matches!(outcome, CommandOutcome::Exit));
    assert!(output.lock().unwrap().shutdown == Shutdown::Complete);
    assert!(state.error.is_none());
    assert_eq!(store.lifetime().unwrap(), 42);
}

#[test]
fn forced_exit_attempts_cleanup_without_waiting_for_success() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    let mut state = State::default();
    state.accounting.total = 42;
    state.accounting.dirty = true;
    let output = Mutex::new(Status::default());

    let outcome = handle_command(&store, Some(Command::Exit), &mut state, &output);

    assert!(matches!(outcome, CommandOutcome::Exit));
    assert_eq!(store.lifetime().unwrap(), 42);
    assert_eq!(output.lock().unwrap().lifetime_bytes, 0);

    state.accounting.total = 50;
    state.accounting.dirty = true;
    std::fs::create_dir(directory.path().join("lifetime.pending")).unwrap();

    let outcome = handle_command(&store, Some(Command::Exit), &mut state, &output);

    assert!(matches!(outcome, CommandOutcome::Exit));
    assert!(state.accounting.dirty);
    assert_eq!(store.lifetime().unwrap(), 42);
}
