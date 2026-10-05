use crate::{
    config::Config,
    sys::{helper::ShutdownReport, nvidia::Redirect},
};
use anyhow::{Context as _, Result};
use std::io::{BufRead, Read, Write};

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub(crate) fn dispatch() -> anyhow::Result<bool> {
    windows::dispatch()
}

pub(super) fn read_redirect(reader: &mut impl BufRead, config: &Config) -> Result<Redirect> {
    let mut bytes = Vec::new();
    reader.take(65_537).read_until(b'\n', &mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= 65_536 && bytes.ends_with(b"\n"),
        "invalid or oversized recovery snapshot"
    );
    let redirect: Redirect = serde_json::from_slice(&bytes).context("decode recovery snapshot")?;
    anyhow::ensure!(
        redirect.target == config.target()
            && redirect.original.path()? == redirect.original_path
            && redirect.original.with_path(&redirect.target) == redirect.replacement,
        "recovery snapshot does not match configuration"
    );
    Ok(redirect)
}

pub(super) fn report_final(
    output: &mut impl Write,
    stopped: ShutdownReport,
    reporting: Result<()>,
) -> Result<()> {
    if let Some(sample) = stopped.sample {
        serde_json::to_writer(&mut *output, &sample)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    stopped.result.context("stop bundled MemFS Extended")?;
    reporting
}

#[cfg(test)]
#[path = "../../tests/unit/filesystem.rs"]
mod tests;
