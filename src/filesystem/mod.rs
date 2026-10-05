use crate::{config::Config, sys::nvidia::Redirect, telemetry::Sample};
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
    sample: Option<&Sample>,
    reporting: Result<()>,
) -> Result<()> {
    if let Some(sample) = sample {
        serde_json::to_writer(&mut *output, sample)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    reporting
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::nvidia::RawValue;
    use std::io::Cursor;

    fn redirect(config: &Config) -> Redirect {
        let original = RawValue {
            kind: 1,
            bytes: vec![],
        }
        .with_path(r"C:\Original");
        Redirect {
            replacement: original.with_path(&config.target()),
            original,
            original_path: r"C:\Original".to_owned(),
            target: config.target(),
        }
    }

    #[test]
    fn snapshot_is_preserved_and_leaves_stop_command_unread() {
        let config = Config::default();
        let expected = redirect(&config);
        let mut bytes = serde_json::to_vec(&expected).unwrap();
        bytes.extend_from_slice(b"\nstop\n");
        let mut input = Cursor::new(bytes);
        let actual = read_redirect(&mut input, &config).unwrap();
        assert_eq!(actual.original, expected.original);
        assert_eq!(actual.replacement, expected.replacement);
        let mut remaining = String::new();
        input.read_to_string(&mut remaining).unwrap();
        assert_eq!(remaining, "stop\n");
    }

    #[test]
    fn rejects_incomplete_oversized_and_mismatched_snapshots() {
        let config = Config::default();
        let mut snapshot = redirect(&config);
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        assert!(read_redirect(&mut Cursor::new(bytes), &config).is_err());
        assert!(read_redirect(&mut Cursor::new(vec![b'x'; 65_537]), &config).is_err());
        snapshot.replacement = snapshot.original.with_path(r"T:\Wrong");
        let mut bytes = serde_json::to_vec(&snapshot).unwrap();
        bytes.push(b'\n');
        assert!(read_redirect(&mut Cursor::new(bytes), &config).is_err());
    }

    #[test]
    fn final_counter_is_forwarded_before_reporting_failure() {
        let sample = Sample {
            version: 1,
            written_bytes: 123,
            buffer_bytes: 0,
            resident_bytes: None,
            available_bytes: 456,
        };
        let mut output = Vec::new();
        let result = report_final(
            &mut output,
            Some(&sample),
            Err(anyhow::anyhow!("helper exited")),
        );
        assert_eq!(result.unwrap_err().to_string(), "helper exited");
        assert_eq!(
            serde_json::from_slice::<Sample>(&output)
                .unwrap()
                .written_bytes,
            123
        );
        assert!(output.ends_with(b"\n"));
    }
}
