use super::{DisplayStatus, Shutdown, State};
use crate::{
    storage::Store,
    sys::{
        helper::ShutdownReport,
        nvidia::{self, Redirect},
    },
    telemetry::Meter,
};
use anyhow::{Context, Result};
use std::time::Instant;

/// Each result retains its source error until the worker reports it to the GUI.
pub(super) struct CleanupReport {
    pub(super) restoration: Result<()>,
    pub(super) helper: Result<()>,
    pub(super) accounting: Result<()>,
    pub(super) persistence: Result<()>,
    pub(super) journal: Result<()>,
}

impl CleanupReport {
    pub(super) fn is_ok(&self) -> bool {
        self.restoration.is_ok()
            && self.helper.is_ok()
            && self.accounting.is_ok()
            && self.persistence.is_ok()
            && self.journal.is_ok()
    }
}

impl std::fmt::Display for CleanupReport {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut separator = "";
        for result in [
            &self.restoration,
            &self.helper,
            &self.accounting,
            &self.persistence,
            &self.journal,
        ] {
            if let Err(error) = result {
                write!(output, "{separator}{error:#}")?;
                separator = "\n";
            }
        }

        Ok(())
    }
}

pub(super) fn stop(store: &Store, state: &mut State) -> CleanupReport {
    stop_with(store, state, |redirect| {
        nvidia::restore(redirect).map_err(Into::into)
    })
}

pub(super) fn stop_with(
    store: &Store,
    state: &mut State,
    restore: impl FnOnce(&Redirect) -> Result<()>,
) -> CleanupReport {
    let Some(mut running) = state.session.take() else {
        let (restoration, journal) = match store.redirect() {
            Ok(Some(redirect)) => {
                let restoration = restore(&redirect).context("recover NVIDIA's original temporary location; retry restoration or set Temporary files to a persistent drive in Alt+Z");
                let journal = if restoration.is_ok() {
                    store
                        .clear_redirect()
                        .context("clear redirect recovery journal")
                } else {
                    Ok(())
                };

                (restoration, journal)
            }
            Ok(None) => (Ok(()), Ok(())),
            Err(error) => (Err(error).context("read redirect recovery journal"), Ok(())),
        };

        let report = CleanupReport {
            restoration,
            helper: Ok(()),
            accounting: Ok(()),
            persistence: state.accounting.persist(store),
            journal,
        };

        if report.is_ok() {
            state.message = DisplayStatus::Stopped;
        }

        return report;
    };

    // Restore before unmounting; retain the journal if restoration or exit fails.
    let restoration = restore(&running.redirect).context("restore NVIDIA temporary path; retry restoration or set Temporary files to a persistent drive in Alt+Z");
    let stopped = running.helper.stop();
    let exited = stopped.exited;
    let report = finish_stop(store, &mut running.meter, state, restoration, stopped);

    if !exited {
        running.stopping = true;
        state.session = Some(running);
    }

    report
}

pub(super) fn finish_stop(
    store: &Store,
    meter: &mut Meter,
    state: &mut State,
    restoration: Result<()>,
    stopped: ShutdownReport,
) -> CleanupReport {
    state.message = if stopped.exited {
        DisplayStatus::CleanupFailed
    } else {
        DisplayStatus::HelperRunning
    };

    // Even a failed helper stop can contain a valid final sample.
    let accounting = if let Some(sample) = &stopped.sample {
        state
            .accounting
            .observe(meter, sample, Instant::now())
            .context("account final RAM writes")
    } else {
        Ok(())
    };
    let persistence = state.accounting.persist(store);
    let journal = if restoration.is_ok() && stopped.exited {
        store
            .clear_redirect()
            .context("clear redirect recovery journal")
    } else {
        Ok(())
    };

    let report = CleanupReport {
        restoration,
        helper: stopped.result.context("stop RAM filesystem"),
        accounting,
        persistence,
        journal,
    };

    if report.is_ok() {
        state.message = DisplayStatus::Stopped;
    }

    report
}

pub(super) fn complete_shutdown(state: &mut State, report: &CleanupReport) {
    if report.is_ok() {
        state.shutdown = Shutdown::Complete;
        state.error = None;
    } else {
        state.shutdown = Shutdown::Failed;
        state.error = Some(format!("Could not finish shutdown: {report}"));
    }
}
