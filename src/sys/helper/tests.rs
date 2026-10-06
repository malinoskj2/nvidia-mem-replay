use super::*;

const SAMPLE: &str =
    r#"{"version":1,"written_bytes":17,"buffer_bytes":0,"resident_bytes":0,"available_bytes":200}"#;

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

fn wait_for_sample(helper: &mut Helper) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while helper.sample().unwrap().is_none() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
}

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

#[test]
fn process_failure_takes_precedence_over_invalid_final_telemetry() {
    let mut helper = helper(&format!(
        "read command; printf '%s\\n' '{SAMPLE}'; printf '%s\\n' 'invalid JSON'; exit 42"
    ));

    let stopped = helper.stop();

    assert_eq!(stopped.sample.unwrap().written_bytes, 17);
    assert!(matches!(stopped.result, Err(HelperError::FailedExit(_))));
}

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

    let error = Helper::wait_ready(command, &Config::default(), None)
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
            assert!(matches!(*error, HelperError::FailedExit(status) if status.code() == Some(42)));
        }
        error => panic!("unexpected shutdown error: {error}"),
    }
}

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
            assert!(matches!(*error, HelperError::FailedExit(status) if status.code() == Some(42)));
            assert_eq!(
                diagnostics.trim().lines().next().unwrap().len(),
                diagnostics::MAX_DIAGNOSTIC_BYTES
            );
            assert!(diagnostics.contains("stderr truncated"));
        }
        error => panic!("unexpected shutdown error: {error}"),
    }
}
