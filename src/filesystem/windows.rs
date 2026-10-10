use crate::{
    config::{self, MAX_MEMORY_LIMIT_MB, MIN_MEMORY_LIMIT_MB, Volume},
    sys::{helper::Helper, icon, nvidia},
};
use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
use std::{
    io::{self, BufRead, Read, Write},
    path::PathBuf,
    sync::mpsc,
    thread,
    time::Duration,
};
use winsafe::{self as w, co, prelude::*};

const TELEMETRY_REPORT_INTERVAL: Duration = Duration::from_millis(250);
const MAX_STOP_COMMAND_BYTES: u64 = 512;

#[derive(Parser)]
#[command(name = crate::APP_NAME)]
struct Cli {
    /// Start hidden in the notification area; the Start-with-Windows entry uses this.
    #[arg(long = "tray")]
    tray: bool,
    #[command(subcommand)]
    command: Option<Mode>,
}

#[derive(Subcommand)]
enum Mode {
    /// Internal owned filesystem process; keep its stdin open until shutdown.
    #[command(hide = true)]
    Filesystem {
        /// `X:` for a drive letter, or the absolute directory to mount the volume at.
        #[arg(long, value_parser = mount_point)]
        mount: String,
        #[arg(long, value_parser = clap::value_parser!(u32).range(i64::from(MIN_MEMORY_LIMIT_MB)..=i64::from(MAX_MEMORY_LIMIT_MB)))]
        memory_limit_mb: u32,
    },
    /// Write the application icon as a Windows `.ico` file (used to refresh `assets/`).
    #[command(hide = true)]
    Icon { path: PathBuf },
}

/// What this process was started to do.
pub(crate) enum Launch {
    /// A helper mode ran to completion.
    Handled,
    /// Show the desktop application, hidden in the tray when `tray` is set.
    Window { tray: bool },
}

/// A drive letter `D:`..`Z:`, or an absolute directory path without `.`/`..` steps or a
/// trailing separator; the native helper checks the same (see [`config::mount_point`]).
fn mount_point(value: &str) -> Result<String, String> {
    config::mount_point(value).map(str::to_owned)
}

pub(super) fn dispatch() -> Result<Launch> {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return report_cli_error(error),
    };
    match cli.command {
        Some(Mode::Filesystem {
            mount,
            memory_limit_mb,
        }) => {
            run(&Volume::new(mount, memory_limit_mb))?;
            Ok(Launch::Handled)
        }
        Some(Mode::Icon { path }) => {
            std::fs::write(&path, icon::ico())
                .with_context(|| format!("write {}", path.display()))?;
            Ok(Launch::Handled)
        }
        None => Ok(Launch::Window { tray: cli.tray }),
    }
}

/// The binary has no console, so clap's output only reaches the user through a message box.
fn report_cli_error(error: clap::Error) -> Result<Launch> {
    let icon = if error.use_stderr() {
        co::MB::ICONERROR
    } else {
        co::MB::ICONINFORMATION
    };
    let _ = w::HWND::NULL.MessageBox(&error.to_string(), crate::APP_NAME, co::MB::OK | icon);

    if error.use_stderr() {
        Err(error.into())
    } else {
        Ok(Launch::Handled)
    }
}

fn run(volume: &Volume) -> Result<()> {
    // The GUI already holds the application's bus identity; a duplicate would be dropped.
    crate::sys::shadowplay::set_role(crate::sys::shadowplay::Role::Supervisor);
    let redirect = super::read_redirect(&mut io::stdin().lock(), &volume.target())?;
    let mut filesystem = Helper::start_memefs(volume).context("start bundled MemFS Extended")?;

    let (stop, receiver) = mpsc::sync_channel(1);
    let reader = thread::Builder::new()
        .name("stdin-stop".to_owned())
        .spawn(move || {
            let mut line = Vec::new();
            let _ = io::stdin()
                .lock()
                .take(MAX_STOP_COMMAND_BYTES)
                .read_until(b'\n', &mut line);
            let _ = stop.send(());
        })
        .context("spawn the stdin stop reader")?;

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
mod tests {
    use super::*;

    // The mount-point rules themselves are covered in `config::tests`; this checks the wiring.

    #[test]
    fn filesystem_cli_accepts_drive_letters_and_absolute_directories() {
        for mount in ["T:", r"C:\Users\me\AppData\Local\NvidiaMemReplay\ram"] {
            assert!(
                Cli::try_parse_from([
                    "replay",
                    "filesystem",
                    "--mount",
                    mount,
                    "--memory-limit-mb",
                    "256"
                ])
                .is_ok(),
                "{mount}"
            );
        }
    }

    #[test]
    fn filesystem_cli_rejects_invalid_or_incomplete_configuration() {
        for args in [
            vec!["replay", "filesystem"],
            vec![
                "replay",
                "filesystem",
                "--mount",
                "C:",
                "--memory-limit-mb",
                "256",
            ],
            vec![
                "replay",
                "filesystem",
                "--mount",
                r"C:\ram\",
                "--memory-limit-mb",
                "256",
            ],
            vec![
                "replay",
                "filesystem",
                "--mount",
                "T:",
                "--memory-limit-mb",
                "0",
            ],
            vec![
                "replay",
                "filesystem",
                "--mount",
                "T:",
                "--memory-limit-mb",
                "65537",
            ],
        ] {
            assert!(Cli::try_parse_from(args.clone()).is_err(), "{args:?}");
        }
    }
}
