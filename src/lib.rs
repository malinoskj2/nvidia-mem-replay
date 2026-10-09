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
/// Pixel size of the window icon handed to the window system.
#[cfg(windows)]
const ICON_SIZE: u32 = 64;

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

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_icon(eframe::egui::IconData {
                rgba: sys::icon::rgba(ICON_SIZE),
                width: ICON_SIZE,
                height: ICON_SIZE,
            })
            .with_visible(!tray)
            .with_inner_size([460.0, 345.0])
            .with_min_inner_size([420.0, 320.0]),
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| match startup {
            Ok((worker, config)) => Ok(Box::new(gui::App::new(cc, worker, config, tray))),
            Err(error) => Ok(Box::new(gui::StartupError::new(cc, format!("{error:#}")))),
        }),
    )
    .map_err(|error| anyhow::anyhow!("open desktop window: {error}"))
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
