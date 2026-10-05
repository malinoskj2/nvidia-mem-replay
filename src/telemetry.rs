use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Sample {
    pub(crate) version: u32,
    pub(crate) written_bytes: u64,
    pub(crate) buffer_bytes: u64,
    pub(crate) resident_bytes: Option<u64>,
    pub(crate) available_bytes: u64,
}

#[derive(Debug, Error)]
pub(crate) enum TelemetryError {
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
        if sample.version != 1 {
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
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(2))
    }
}

#[cfg(test)]
mod tests {
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
}
