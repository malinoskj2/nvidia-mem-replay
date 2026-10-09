use super::*;
use crate::config::Config;
use crate::sys::nvidia::RawValue;
use crate::{sys::helper::HelperError, telemetry::Sample};
use std::io::Cursor;

fn redirect(config: &Config) -> Redirect {
    let original = RawValue {
        kind: 1,
        bytes: vec![],
    }
    .with_path(r"C:\Original");

    Redirect {
        replacement: original.with_path(&config.target()),
        original,
        original_path: r"C:\Original".to_owned(),
        target: config.target(),
    }
}

#[test]
fn snapshot_is_preserved_and_leaves_stop_command_unread() {
    let config = Config::default();
    let expected = redirect(&config);
    let mut bytes = serde_json::to_vec(&expected).unwrap();
    bytes.extend_from_slice(b"\nstop\n");
    let mut input = Cursor::new(bytes);

    let actual = read_redirect(&mut input, &config.target()).unwrap();

    assert_eq!(actual.original, expected.original);
    assert_eq!(actual.replacement, expected.replacement);

    let mut remaining = String::new();
    input.read_to_string(&mut remaining).unwrap();
    assert_eq!(remaining, "stop\n");
}

#[test]
fn rejects_incomplete_oversized_and_mismatched_snapshots() {
    let config = Config::default();
    let mut snapshot = redirect(&config);
    let bytes = serde_json::to_vec(&snapshot).unwrap();

    let target = config.target();
    assert!(read_redirect(&mut Cursor::new(bytes), &target).is_err());
    assert!(
        read_redirect(
            &mut Cursor::new(vec![b'x'; MAX_RECOVERY_SNAPSHOT_BYTES + 1]),
            &target
        )
        .is_err()
    );

    let mut other_volume = serde_json::to_vec(&snapshot).unwrap();
    other_volume.push(b'\n');
    assert!(read_redirect(&mut Cursor::new(other_volume), r"T:\Other").is_err());

    snapshot.replacement = snapshot.original.with_path(r"T:\Wrong");
    let mut bytes = serde_json::to_vec(&snapshot).unwrap();
    bytes.push(b'\n');

    assert!(read_redirect(&mut Cursor::new(bytes), &target).is_err());
}

#[test]
fn final_counter_is_forwarded_before_reporting_failure() {
    let sample = Sample {
        version: crate::telemetry::TELEMETRY_PROTOCOL_VERSION,
        written_bytes: 123,
        buffer_bytes: 0,
        resident_bytes: None,
        available_bytes: 456,
    };
    let mut output = Vec::new();

    let result = report_final(
        &mut output,
        ShutdownReport {
            sample: Some(sample),
            result: Ok(()),
            exited: true,
        },
        Err(anyhow::anyhow!("helper exited")),
    );

    assert_eq!(result.unwrap_err().to_string(), "helper exited");
    assert_eq!(
        serde_json::from_slice::<Sample>(&output)
            .unwrap()
            .written_bytes,
        123
    );
    assert!(output.ends_with(b"\n"));
}

#[test]
fn final_counter_is_forwarded_before_shutdown_failure() {
    let sample = Sample {
        version: crate::telemetry::TELEMETRY_PROTOCOL_VERSION,
        written_bytes: 123,
        buffer_bytes: 0,
        resident_bytes: None,
        available_bytes: 456,
    };
    let mut output = Vec::new();

    let result = report_final(
        &mut output,
        ShutdownReport {
            sample: Some(sample),
            result: Err(HelperError::ForcedTermination),
            exited: true,
        },
        Ok(()),
    );

    assert!(matches!(
        result.unwrap_err().downcast_ref::<HelperError>(),
        Some(HelperError::ForcedTermination)
    ));
    assert_eq!(
        serde_json::from_slice::<Sample>(&output)
            .unwrap()
            .written_bytes,
        123
    );
    assert!(output.ends_with(b"\n"));
}
