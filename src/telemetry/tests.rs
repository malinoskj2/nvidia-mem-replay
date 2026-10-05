use super::*;

fn sample(bytes: u64) -> Sample {
    Sample {
        version: TELEMETRY_PROTOCOL_VERSION,
        written_bytes: bytes,
        buffer_bytes: 0,
        resident_bytes: Some(0),
        available_bytes: 0,
    }
}

#[test]
fn cumulative_writes_survive_overwrites_deletion_and_dropped_samples() {
    let now = Instant::now();
    let mut meter = Meter::new(8_000);
    assert_eq!(meter.observe(&sample(100), now).unwrap(), 8_100);
    assert_eq!(meter.observe(&sample(900), now).unwrap(), 8_900);
    assert!(meter.active(now));
    assert!(!meter.active(now + Duration::from_secs(3)));
    assert!(matches!(
        meter.observe(&sample(1), now),
        Err(TelemetryError::CounterReset)
    ));
}

#[test]
fn unsupported_protocol_and_overflow_are_errors() {
    let mut meter = Meter::new(u64::MAX);
    assert!(matches!(
        meter.observe(&sample(1), Instant::now()),
        Err(TelemetryError::Overflow)
    ));
    let mut wrong = sample(0);
    wrong.version = 2;
    assert!(matches!(
        meter.observe(&wrong, Instant::now()),
        Err(TelemetryError::Version(2))
    ));
}

#[test]
fn decodes_complete_sample_and_accepts_the_frame_size_boundary() {
    let mut frame = serde_json::to_vec(&sample(17)).unwrap();
    frame.resize(MAX_TELEMETRY_FRAME_BYTES - 1, b' ');
    frame.push(b'\n');
    let decoded = decode_sample(&frame).unwrap();
    assert_eq!(decoded.version, TELEMETRY_PROTOCOL_VERSION);
    assert_eq!(decoded.written_bytes, 17);
    assert_eq!(decoded.resident_bytes, Some(0));
}

#[test]
fn rejects_malformed_and_incomplete_samples() {
    for frame in [
        b"invalid JSON\n".as_slice(),
        b"{\"version\":1}\n".as_slice(),
        b"{\"version\":1".as_slice(),
    ] {
        assert!(decode_sample(frame).is_err());
    }
    let complete_json = serde_json::to_vec(&sample(0)).unwrap();
    assert_eq!(
        decode_sample(&complete_json).unwrap_err(),
        "invalid or oversized helper telemetry"
    );
}

#[test]
fn rejects_oversized_sample_frames() {
    let mut frame = serde_json::to_vec(&sample(0)).unwrap();
    frame.resize(MAX_TELEMETRY_FRAME_BYTES, b' ');
    frame.push(b'\n');
    assert_eq!(
        decode_sample(&frame).unwrap_err(),
        "invalid or oversized helper telemetry"
    );
}

#[test]
fn rejects_unsupported_sample_protocol() {
    let mut unsupported = sample(0);
    unsupported.version = TELEMETRY_PROTOCOL_VERSION + 1;
    let mut frame = serde_json::to_vec(&unsupported).unwrap();
    frame.push(b'\n');
    assert_eq!(
        decode_sample(&frame).unwrap_err(),
        format!("unsupported protocol version {}", unsupported.version)
    );
}
