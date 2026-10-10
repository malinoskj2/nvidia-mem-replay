use crate::{
    storage::Store,
    telemetry::{Meter, Sample},
};
use anyhow::{Context, Result};
use std::time::{Duration, Instant};

const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(10);
const CHECKPOINT_WARNING: &str =
    "Lifetime counter could not be saved; RAM recording continues. Retrying in 10 seconds";
const SAVE_LIFETIME_CONTEXT: &str =
    "save lifetime write counter; check application state directory access and retry";

#[derive(Default)]
pub(super) struct Accounting {
    pub(super) total: u64,
    pub(super) dirty: bool,
    pub(super) warning: Option<String>,
    last_attempt: Option<Instant>,
}

impl Accounting {
    /// Starts the checkpoint cadence so the first persist attempt follows the session start by
    /// `CHECKPOINT_INTERVAL`.
    pub(super) fn begin_session(&mut self, now: Instant) {
        self.last_attempt = Some(now);
    }

    pub(super) fn load(&mut self, store: &Store) -> Result<()> {
        // Unsaved writes remain counted across a session restart.
        if !self.dirty {
            self.total = store.lifetime()?;
        }

        Ok(())
    }

    pub(super) fn observe(
        &mut self,
        meter: &mut Meter,
        sample: &Sample,
        now: Instant,
    ) -> Result<()> {
        let total = meter.observe(sample, now)?;
        self.dirty |= total != self.total;
        self.total = total;
        Ok(())
    }

    pub(super) fn checkpoint(&mut self, store: &Store, now: Instant) {
        let last_attempt = self.last_attempt.get_or_insert(now);
        if now.duration_since(*last_attempt) < CHECKPOINT_INTERVAL {
            return;
        }

        // Retry failures at the same bounded cadence while recording continues.
        *last_attempt = now;
        if let Err(error) = self.persist(store) {
            self.warning = Some(format!("{CHECKPOINT_WARNING}: {error:#}"));
        }
    }

    pub(super) fn persist(&mut self, store: &Store) -> Result<()> {
        if self.dirty {
            store
                .save_lifetime(self.total)
                .context(SAVE_LIFETIME_CONTEXT)?;
            self.dirty = false;
            self.warning = None;
        }

        Ok(())
    }
}
