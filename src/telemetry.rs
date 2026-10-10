use serde::{Deserialize, Serialize};
use std::{io, time::Duration};
use thiserror::Error;

pub(crate) const TELEMETRY_PROTOCOL_VERSION: u32 = 1;
pub(crate) const MAX_TELEMETRY_FRAME_BYTES: usize = 1024;
/// The filesystem reports every 10 s and the supervisor forwards on its own 10 s cadence, so a
/// frame can take about 20 s to arrive.
pub(crate) const TELEMETRY_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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
    advanced: bool,
}

impl Meter {
    pub(crate) const fn new(base: u64) -> Self {
        Self {
            base,
            last: 0,
            advanced: false,
        }
    }

    pub(crate) fn observe(&mut self, sample: &Sample) -> Result<u64, TelemetryError> {
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

        self.advanced = sample.written_bytes > self.last;
        self.last = sample.written_bytes;

        Ok(total)
    }

    /// Whether the most recently observed sample advanced the write counter.
    pub(crate) const fn active(&self) -> bool {
        self.advanced
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::assert_matches;

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
    fn cumulative_counters_include_skipped_samples_and_report_whether_the_latest_advanced() {
        let mut meter = Meter::new(8_000);
        assert!(!meter.active());

        assert_eq!(meter.observe(&sample(100)).unwrap(), 8_100);
        assert!(meter.active());
        assert_eq!(meter.observe(&sample(900)).unwrap(), 8_900);
        assert!(meter.active());
        assert_eq!(meter.observe(&sample(900)).unwrap(), 8_900);
        assert!(!meter.active());
        assert_matches!(meter.observe(&sample(1)), Err(TelemetryError::CounterReset));
    }

    #[test]
    fn unsupported_protocol_and_overflow_are_errors() {
        let mut meter = Meter::new(u64::MAX);
        assert_matches!(meter.observe(&sample(1)), Err(TelemetryError::Overflow));

        let mut wrong = sample(0);
        wrong.version = 2;
        assert_matches!(meter.observe(&wrong), Err(TelemetryError::Version(2)));
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
            assert_matches!(decode_sample(frame), Err(TelemetryError::Json(_)));
        }

        let complete_json = serde_json::to_vec(&sample(0)).unwrap();

        assert_matches!(decode_sample(&complete_json), Err(TelemetryError::Frame));
    }

    #[test]
    fn rejects_oversized_sample_frames() {
        let mut frame = serde_json::to_vec(&sample(0)).unwrap();
        frame.resize(MAX_TELEMETRY_FRAME_BYTES, b' ');
        frame.push(b'\n');

        assert_matches!(decode_sample(&frame), Err(TelemetryError::Frame));
    }

    #[test]
    fn rejects_unsupported_sample_protocol() {
        let mut unsupported = sample(0);
        unsupported.version = TELEMETRY_PROTOCOL_VERSION + 1;
        let mut frame = serde_json::to_vec(&unsupported).unwrap();
        frame.push(b'\n');

        assert_matches!(
            decode_sample(&frame),
            Err(TelemetryError::Version(version)) if version == unsupported.version
        );
    }
}
