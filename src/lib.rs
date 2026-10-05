#![allow(dead_code)]

mod config;
mod storage;
mod sys;
mod telemetry;

use anyhow::Result;

/// The recording application is available on Windows only.
pub fn run() -> Result<()> {
    anyhow::bail!("Replay in RAM requires Windows x64, NVIDIA overlay, and WinFsp")
}
