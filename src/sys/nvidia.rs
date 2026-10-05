use serde::{Deserialize, Serialize};
use std::io;
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct RawValue {
    pub(crate) kind: u32,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Redirect {
    pub(crate) original: RawValue,
    pub(crate) replacement: RawValue,
    pub(crate) original_path: String,
    pub(crate) target: String,
}

#[derive(Debug, Error)]
pub(crate) enum NvidiaError {
    #[error(
        "NVIDIA TempFilePath was not found. Enable the NVIDIA overlay and set Temporary files in Alt+Z > Settings > Files and disk space, then retry"
    )]
    #[cfg_attr(not(windows), allow(dead_code))]
    Missing,
    #[error("NVIDIA temporary path has an unsupported registry format")]
    Format,
    #[error("NVIDIA temporary path is empty or invalid")]
    Path,
    #[error(
        "NVIDIA already targets the selected RAM drive without a recovery journal. Set a persistent temporary-files location in the overlay, then retry"
    )]
    RamOriginal,
    #[error("NVIDIA registry access: {0}")]
    Io(#[from] io::Error),
    #[error("NVIDIA changed its temporary path while this operation was in progress; retry")]
    Changed,
    #[error("this app requires Windows and the NVIDIA overlay")]
    #[cfg(not(windows))]
    Unsupported,
}

impl RawValue {
    pub(crate) fn path(&self) -> Result<String, NvidiaError> {
        // NVIDIA versions use both REG_SZ and UTF-16 REG_BINARY.
        if !matches!(self.kind, 1..=3)
            || !self.bytes.len().is_multiple_of(2)
            || self.bytes.len() > 65536
        {
            return Err(NvidiaError::Format);
        }
        let mut words: Vec<u16> = self
            .bytes
            .chunks_exact(2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
            .collect();
        while words.last() == Some(&0) {
            words.pop();
        }
        let path = String::from_utf16(&words).map_err(|_| NvidiaError::Format)?;
        let drive_path = path.as_bytes().get(1..3) == Some(b":\\")
            && path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
        let unc_path = path.starts_with(r"\\");
        let expandable_path = self.kind == 2 && path.starts_with('%') && path[1..].contains('%');
        if path.contains('\0') || !(drive_path || unc_path || expandable_path) {
            return Err(NvidiaError::Path);
        }
        Ok(path)
    }

    pub(crate) fn with_path(&self, path: &str) -> Self {
        let bytes = path
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        Self {
            kind: self.kind,
            bytes,
        }
    }
}

pub(crate) fn plan(target: String) -> Result<Redirect, NvidiaError> {
    let original = read()?;
    let original_path = original.path()?;
    if original_path
        .get(..2)
        .zip(target.get(..2))
        .is_some_and(|(original, target)| original.eq_ignore_ascii_case(target))
    {
        return Err(NvidiaError::RamOriginal);
    }
    Ok(Redirect {
        replacement: original.with_path(&target),
        original,
        original_path,
        target,
    })
}

pub(crate) fn apply(redirect: &Redirect) -> Result<(), NvidiaError> {
    if read()? != redirect.original {
        return Err(NvidiaError::Changed);
    }
    write(&redirect.replacement)
}

impl Redirect {
    fn restore_value(&self, current: &RawValue) -> Option<&RawValue> {
        (current == &self.replacement).then_some(&self.original)
    }
}

/// Only restore our own value; a later user/overlay edit takes precedence.
pub(crate) fn restore(redirect: &Redirect) -> Result<(), NvidiaError> {
    match read() {
        Ok(value) => redirect.restore_value(&value).map_or(Ok(()), write),
        Err(NvidiaError::Missing) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
const KEY: &str = r"Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS";

#[cfg(windows)]
fn read() -> Result<RawValue, NvidiaError> {
    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WOW64_64KEY},
    };
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(KEY, KEY_READ | KEY_WOW64_64KEY)
        .map_err(registry_error)?;
    let value = key.get_raw_value("TempFilePath").map_err(registry_error)?;
    Ok(RawValue {
        kind: value.vtype as u32,
        bytes: value.bytes.into_owned(),
    })
}

#[cfg(windows)]
fn write(value: &RawValue) -> Result<(), NvidiaError> {
    use winreg::{
        RegKey, RegValue,
        enums::{HKEY_CURRENT_USER, KEY_WOW64_64KEY, KEY_WRITE, RegType},
    };
    let kind = match value.kind {
        1 => RegType::REG_SZ,
        2 => RegType::REG_EXPAND_SZ,
        3 => RegType::REG_BINARY,
        _ => return Err(NvidiaError::Format),
    };
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(KEY, KEY_WRITE | KEY_WOW64_64KEY)?;
    key.set_raw_value(
        "TempFilePath",
        &RegValue {
            vtype: kind,
            bytes: std::borrow::Cow::Borrowed(&value.bytes),
        },
    )?;
    Ok(())
}

#[cfg(windows)]
fn registry_error(error: io::Error) -> NvidiaError {
    if error.kind() == io::ErrorKind::NotFound {
        NvidiaError::Missing
    } else {
        NvidiaError::Io(error)
    }
}

#[cfg(not(windows))]
const fn read() -> Result<RawValue, NvidiaError> {
    Err(NvidiaError::Unsupported)
}

#[cfg(not(windows))]
const fn write(_: &RawValue) -> Result<(), NvidiaError> {
    Err(NvidiaError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_and_string_paths_preserve_the_registry_type() {
        for kind in [1, 2, 3] {
            let value = RawValue {
                kind,
                bytes: vec![],
            }
            .with_path(r"C:\Temp\影像");
            assert_eq!(value.path().unwrap(), r"C:\Temp\影像");
            assert_eq!(value.with_path(r"R:\NVIDIA-Replay").kind, kind);
        }
    }

    #[test]
    fn rejects_malformed_or_empty_paths() {
        assert!(
            RawValue {
                kind: 3,
                bytes: vec![1]
            }
            .path()
            .is_err()
        );
        assert!(
            RawValue {
                kind: 1,
                bytes: vec![0, 0]
            }
            .path()
            .is_err()
        );
        assert!(
            RawValue {
                kind: 4,
                bytes: vec![65, 0]
            }
            .path()
            .is_err()
        );
    }

    #[test]
    fn restoration_preserves_user_edits_and_original_binary_bytes() {
        let original = RawValue {
            kind: 3,
            bytes: vec![],
        }
        .with_path(r"C:\Temp");
        let replacement = original.with_path(r"R:\NVIDIA-Replay");
        let redirect = Redirect {
            original: original.clone(),
            replacement: replacement.clone(),
            original_path: r"C:\Temp".to_owned(),
            target: r"R:\NVIDIA-Replay".to_owned(),
        };
        assert_eq!(redirect.restore_value(&replacement), Some(&original));
        assert_eq!(
            redirect.restore_value(&original.with_path(r"D:\NewTemp")),
            None
        );
        assert_eq!(redirect.restore_value(&original), None);
    }
}
