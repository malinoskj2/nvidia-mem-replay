#![allow(dead_code)]

mod config;
mod filesystem;
mod service;
mod storage;
mod sys;
mod telemetry;

use anyhow::Result;

/// Start the owned filesystem supervisor on Windows.
#[cfg(windows)]
pub fn run() -> Result<()> {
    if filesystem::dispatch()? {
        return Ok(());
    }
    anyhow::bail!("Desktop controls are not available yet")
}

/// The recording application is available on Windows only.
#[cfg(not(windows))]
pub fn run() -> Result<()> {
    anyhow::bail!("Replay in RAM requires Windows x64, NVIDIA overlay, and WinFsp")
}
