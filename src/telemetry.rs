use serde::{Deserialize, Serialize};
use std::{
    io,
    time::{Duration, Instant},
};
use thiserror::Error;

pub(crate) const TELEMETRY_PROTOCOL_VERSION: u32 = 1;
pub(crate) const MAX_TELEMETRY_FRAME_BYTES: usize = 1024;
pub(crate) const TELEMETRY_TIMEOUT: Duration = Duration::from_secs(3);
const WRITE_ACTIVITY_WINDOW: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Sample {
    pub(crate) version: u32,
    pub(crate) written_bytes: u64,
    pub(crate) buffer_bytes: u64,
    pub(crate) resident_bytes: Option<u64>,
    pub(crate) available_bytes: u64,
}

pub(crate) fn decode_sample(frame: &[u8]) -> Result<Sample, TelemetryError> {
    if frame.len() > MAX_TELEMETRY_FRAME_BYTES || !frame.ends_with(b"\n") {
        return Err(TelemetryError::Frame);
    }

    let sample: Sample = serde_json::from_slice(frame)?;
    if sample.version != TELEMETRY_PROTOCOL_VERSION {
        return Err(TelemetryError::Version(sample.version));
    }

    Ok(sample)
}

#[derive(Debug, Error)]
pub(crate) enum TelemetryError {
    #[error("invalid or oversized helper telemetry")]
    Frame,
    #[error("invalid helper telemetry JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("helper telemetry I/O: {0}")]
    Io(#[from] io::Error),
    #[error("unsupported helper telemetry version {0}; rebuild the bundled helper")]
    Version(u32),
    #[error("helper write counter decreased within a session")]
    CounterReset,
    #[error("lifetime byte counter overflow")]
    Overflow,
}

pub(crate) struct Meter {
    base: u64,
    last: u64,
    activity: Option<Instant>,
}

impl Meter {
    pub(crate) const fn new(base: u64) -> Self {
        Self {
            base,
            last: 0,
            activity: None,
        }
    }

    pub(crate) fn observe(&mut self, sample: &Sample, now: Instant) -> Result<u64, TelemetryError> {
        if sample.version != TELEMETRY_PROTOCOL_VERSION {
            return Err(TelemetryError::Version(sample.version));
        }
        if sample.written_bytes < self.last {
            return Err(TelemetryError::CounterReset);
        }

        let total = self
            .base
            .checked_add(sample.written_bytes)
            .ok_or(TelemetryError::Overflow)?;

        if sample.written_bytes > self.last {
            self.activity = Some(now);
        }
        self.last = sample.written_bytes;

        Ok(total)
    }

    pub(crate) fn active(&self, now: Instant) -> bool {
        self.activity
            .is_some_and(|last| now.duration_since(last) < WRITE_ACTIVITY_WINDOW)
    }
}

#[cfg(test)]
mod tests {
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
    fn cumulative_counters_include_skipped_samples_and_track_recent_activity() {
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
            b"{\"version\":1\n".as_slice(),
        ] {
            assert!(matches!(decode_sample(frame), Err(TelemetryError::Json(_))));
        }

        let complete_json = serde_json::to_vec(&sample(0)).unwrap();

        assert!(matches!(
            decode_sample(&complete_json),
            Err(TelemetryError::Frame)
        ));
    }

    #[test]
    fn rejects_oversized_sample_frames() {
        let mut frame = serde_json::to_vec(&sample(0)).unwrap();
        frame.resize(MAX_TELEMETRY_FRAME_BYTES, b' ');
        frame.push(b'\n');

        assert!(matches!(decode_sample(&frame), Err(TelemetryError::Frame)));
    }

    #[test]
    fn rejects_unsupported_sample_protocol() {
        let mut unsupported = sample(0);
        unsupported.version = TELEMETRY_PROTOCOL_VERSION + 1;
        let mut frame = serde_json::to_vec(&unsupported).unwrap();
        frame.push(b'\n');

        assert!(matches!(
            decode_sample(&frame),
            Err(TelemetryError::Version(version)) if version == unsupported.version
        ));
    }
}
