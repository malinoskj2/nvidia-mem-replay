use crate::{
    config::{Config, MAX_DRIVE, MAX_MEMORY_LIMIT_MB, MIN_DRIVE, MIN_MEMORY_LIMIT_MB},
    sys::{helper::Helper, nvidia},
};
use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use std::{
    io::{self, BufRead, Read, Write},
    sync::mpsc,
    thread,
    time::Duration,
};

const TELEMETRY_REPORT_INTERVAL: Duration = Duration::from_millis(250);
const MAX_STOP_COMMAND_BYTES: u64 = 512;

#[derive(Parser)]
#[command(name = crate::APP_NAME)]
struct Cli {
    #[command(subcommand)]
    command: Option<Mode>,
}

#[derive(Subcommand)]
enum Mode {
    /// Internal owned filesystem process; keep its stdin open until shutdown.
    #[command(hide = true)]
    Filesystem {
        #[arg(long, value_parser = drive)]
        drive: char,
        #[arg(long, value_parser = clap::value_parser!(u32).range(i64::from(MIN_MEMORY_LIMIT_MB)..=i64::from(MAX_MEMORY_LIMIT_MB)))]
        memory_limit_mb: u32,
    },
}

fn drive(value: &str) -> Result<char, String> {
    let mut chars = value.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), None) if (MIN_DRIVE..=MAX_DRIVE).contains(&letter) => Ok(letter),
        _ => Err("drive must be one letter D through Z".to_owned()),
    }
}

pub(super) fn dispatch() -> Result<bool> {
    let Some(Mode::Filesystem {
        drive,
        memory_limit_mb,
    }) = Cli::try_parse()?.command
    else {
        return Ok(false);
    };

    run(&Config {
        drive,
        memory_limit_mb,
    })?;
    Ok(true)
}

fn run(config: &Config) -> Result<()> {
    let redirect = super::read_redirect(&mut io::stdin().lock(), config)?;
    let mut filesystem = Helper::start_memefs(config).context("start bundled MemFS Extended")?;

    let (stop, receiver) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let mut line = Vec::new();
        let _ = io::stdin()
            .lock()
            .take(MAX_STOP_COMMAND_BYTES)
            .read_until(b'\n', &mut line);
        let _ = stop.send(());
    });

    let mut stdout = io::stdout().lock();
    let reporting = (|| -> Result<()> {
        loop {
            if let Some(sample) = filesystem.sample()? {
                serde_json::to_writer(&mut stdout, &sample)?;
                stdout.write_all(b"\n")?;
                stdout.flush()?;
            }

            if !matches!(
                receiver.recv_timeout(TELEMETRY_REPORT_INTERVAL),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                break;
            }
        }
        Ok(())
    })();

    // GUI EOF includes crashes. Restore the exact original before dropping RAM.
    let restored = nvidia::restore(&redirect);
    let stopped = filesystem.stop();
    if reader.is_finished() {
        let _ = reader.join();
    }

    // Forward completed writes even when reporting or restoration failed.
    let final_report = super::report_final(&mut stdout, stopped, reporting);
    restored?;
    final_report?;
    Ok(())
}

#[cfg(test)]
mod tests;
