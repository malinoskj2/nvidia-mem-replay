use super::*;

fn sample(bytes: u64) -> Sample {
    Sample {
        version: 1,
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
