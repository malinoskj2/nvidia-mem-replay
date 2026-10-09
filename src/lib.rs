#![cfg_attr(not(windows), allow(dead_code))]

mod config;
mod filesystem;
#[cfg(windows)]
mod gui;
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

    let startup = (|| -> Result<_> {
        let store = storage::Store::open().context("open application state")?;
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
mod tests;
