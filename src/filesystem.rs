use crate::sys::{helper::ShutdownReport, nvidia::Redirect};
use anyhow::{Context as _, Result};
use std::io::{BufRead, Read, Write};

// The snapshot frame includes the trailing newline sent by the GUI.
pub(crate) const MAX_RECOVERY_SNAPSHOT_BYTES: usize = 65_536;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub(crate) use windows::Launch;

#[cfg(windows)]
pub(crate) fn dispatch() -> anyhow::Result<Launch> {
    windows::dispatch()
}

/// Reads the GUI's recovery snapshot, which must redirect NVIDIA to this volume's `target`.
pub(super) fn read_redirect(reader: &mut impl BufRead, target: &str) -> Result<Redirect> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_RECOVERY_SNAPSHOT_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= MAX_RECOVERY_SNAPSHOT_BYTES && bytes.ends_with(b"\n"),
        "invalid or oversized recovery snapshot"
    );

    let redirect: Redirect = serde_json::from_slice(&bytes).context("decode recovery snapshot")?;
    anyhow::ensure!(
        redirect.target == target
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
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::sys::nvidia::RawValue;
    use crate::{sys::helper::HelperError, telemetry::Sample};
    use std::assert_matches;
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

        let actual = read_redirect(&mut input, &config.target()).unwrap();

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

        let target = config.target();
        assert!(read_redirect(&mut Cursor::new(bytes), &target).is_err());
        assert!(
            read_redirect(
                &mut Cursor::new(vec![b'x'; MAX_RECOVERY_SNAPSHOT_BYTES + 1]),
                &target
            )
            .is_err()
        );

        let mut other_volume = serde_json::to_vec(&snapshot).unwrap();
        other_volume.push(b'\n');
        assert!(read_redirect(&mut Cursor::new(other_volume), r"T:\Other").is_err());

        snapshot.replacement = snapshot.original.with_path(r"T:\Wrong");
        let mut bytes = serde_json::to_vec(&snapshot).unwrap();
        bytes.push(b'\n');

        assert!(read_redirect(&mut Cursor::new(bytes), &target).is_err());
    }

    #[test]
    fn final_counter_is_forwarded_before_reporting_failure() {
        let sample = Sample {
            version: crate::telemetry::TELEMETRY_PROTOCOL_VERSION,
            written_bytes: 123,
            buffer_bytes: 0,
            resident_bytes: None,
            available_bytes: 456,
        };
        let mut output = Vec::new();

        let result = report_final(
            &mut output,
            ShutdownReport {
                sample: Some(sample),
                result: Ok(()),
                exited: true,
            },
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

    #[test]
    fn final_counter_is_forwarded_before_shutdown_failure() {
        let sample = Sample {
            version: crate::telemetry::TELEMETRY_PROTOCOL_VERSION,
            written_bytes: 123,
            buffer_bytes: 0,
            resident_bytes: None,
            available_bytes: 456,
        };
        let mut output = Vec::new();

        let result = report_final(
            &mut output,
            ShutdownReport {
                sample: Some(sample),
                result: Err(HelperError::ForcedTermination),
                exited: true,
            },
            Ok(()),
        );

        assert_matches!(
            result.unwrap_err().downcast_ref::<HelperError>(),
            Some(HelperError::ForcedTermination)
        );
        assert_eq!(
            serde_json::from_slice::<Sample>(&output)
                .unwrap()
                .written_bytes,
            123
        );
        assert!(output.ends_with(b"\n"));
    }
}
