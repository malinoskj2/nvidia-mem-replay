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
mod tests;
