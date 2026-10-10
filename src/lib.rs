#![cfg_attr(not(windows), allow(dead_code))]

mod config;
mod filesystem;
#[cfg(windows)]
mod gui;
mod log;
mod service;
mod storage;
mod sys;
mod telemetry;

pub(crate) const APP_NAME: &str = "nvidia-mem-replay";

#[cfg(not(windows))]
const UNSUPPORTED_PLATFORM_MESSAGE: &str =
    "nvidia-mem-replay requires Windows x64, NVIDIA overlay, and WinFsp";

#[cfg(any(windows, test))]
use anyhow::Context;
use anyhow::Result;

/// Start the desktop application and its owned recording worker.
#[cfg(windows)]
pub fn run() -> Result<()> {
    let tray = match filesystem::dispatch()? {
        filesystem::Launch::Handled => return Ok(()),
        filesystem::Launch::Window { tray } => tray,
    };

    log::info(format!(
        "{APP_NAME} {} starting on {}",
        env!("CARGO_PKG_VERSION"),
        log::today()
    ));
    let startup = (|| -> Result<_> {
        let store = storage::Store::open().context("open application state")?;
        let log_file = storage::state_directory().join(log::FILE_NAME);
        if let Err(error) = log::mirror_to(&log_file) {
            log::warning(format!(
                "The log file {} could not be created: {error}",
                log_file.display()
            ));
        }
        let config = startup_config(&store, |redirect| {
            sys::nvidia::restore(redirect).map_err(Into::into)
        })?;
        let worker = service::Worker::spawn(store, config.clone());
        Ok((worker, config))
    })();

    gui::run(startup, tray)
}

/// The recording application is available on Windows only.
#[cfg(not(windows))]
pub fn run() -> Result<()> {
    anyhow::bail!(UNSUPPORTED_PLATFORM_MESSAGE)
}

// Recovery is independent of parsing configuration or lifetime state.
#[cfg(any(windows, test))]
fn startup_config(
    store: &storage::Store,
    restore: impl FnOnce(&sys::nvidia::Redirect) -> Result<()>,
) -> Result<config::Config> {
    service::recover_with(store, restore)?;
    store.load_config().context("load configuration")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::nvidia::{RawValue, Redirect};

    fn pending_redirect() -> Redirect {
        let original = RawValue {
            kind: 1,
            bytes: Vec::new(),
        }
        .with_path(r"C:\NVIDIA");

        Redirect {
            replacement: original.with_path(r"R:\Temp"),
            original,
            original_path: r"C:\NVIDIA".to_owned(),
            target: r"R:\Temp".to_owned(),
        }
    }

    #[test]
    fn pending_recovery_precedes_malformed_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let store = storage::Store::at(directory.path().to_owned()).unwrap();
        store.save_redirect(&pending_redirect()).unwrap();
        std::fs::write(directory.path().join("config.json"), b"broken").unwrap();
        let mut restored = false;

        let result = startup_config(&store, |redirect| {
            assert_eq!(redirect.original_path, r"C:\NVIDIA");
            restored = true;
            Ok(())
        });

        assert!(restored);
        assert!(result.is_err());
        assert!(store.redirect().unwrap().is_none());
    }

    #[test]
    fn failed_recovery_retains_journal_and_can_retry_before_loading_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let store = storage::Store::at(directory.path().to_owned()).unwrap();
        store.save_redirect(&pending_redirect()).unwrap();
        std::fs::write(directory.path().join("config.json"), b"broken").unwrap();

        let failure = startup_config(&store, |_| anyhow::bail!("registry unavailable"));

        assert!(format!("{:#}", failure.unwrap_err()).contains("registry unavailable"));
        assert!(store.redirect().unwrap().is_some());

        assert!(startup_config(&store, |_| Ok(())).is_err());
        assert!(store.redirect().unwrap().is_none());
    }
}
