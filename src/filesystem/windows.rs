use crate::{
    config::Config,
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

#[derive(Parser)]
#[command(name = "Replay in RAM")]
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
        #[arg(long, value_parser = clap::value_parser!(u32).range(256..=65536))]
        memory_limit_mb: u32,
    },
}

fn drive(value: &str) -> Result<char, String> {
    let mut chars = value.chars();
    match (chars.next(), chars.next()) {
        (Some(letter @ 'D'..='Z'), None) => Ok(letter),
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
    config.validate()?;
    let redirect = nvidia::plan(config.target()).context("snapshot original NVIDIA location")?;
    let mut filesystem = Helper::start_memefs(config).context("start bundled MemFS Extended")?;
    let (stop, receiver) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let mut line = Vec::new();
        let _ = io::stdin().lock().take(512).read_until(b'\n', &mut line);
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
                receiver.recv_timeout(Duration::from_millis(250)),
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
    restored?;
    reporting?;
    if let Some(sample) = stopped? {
        serde_json::to_writer(&mut stdout, &sample)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_cli_rejects_invalid_or_incomplete_configuration() {
        assert!(
            Cli::try_parse_from([
                "replay",
                "filesystem",
                "--drive",
                "T",
                "--memory-limit-mb",
                "256"
            ])
            .is_ok()
        );
        for args in [
            vec!["replay", "filesystem"],
            vec![
                "replay",
                "filesystem",
                "--drive",
                "C",
                "--memory-limit-mb",
                "256",
            ],
            vec![
                "replay",
                "filesystem",
                "--drive",
                "TT",
                "--memory-limit-mb",
                "256",
            ],
            vec![
                "replay",
                "filesystem",
                "--drive",
                "T",
                "--memory-limit-mb",
                "0",
            ],
            vec![
                "replay",
                "filesystem",
                "--drive",
                "T",
                "--memory-limit-mb",
                "65537",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }
}
