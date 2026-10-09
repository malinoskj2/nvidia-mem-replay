use serde::{Deserialize, Serialize};
use std::io;
use thiserror::Error;

#[cfg(windows)]
use super::shadowplay;

const MAX_REGISTRY_VALUE_BYTES: usize = 64 * 1024;

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
        "NVIDIA already targets the RAM storage location without a recovery journal. Set a persistent temporary-files location in the overlay, then retry"
    )]
    RamOriginal,
    #[error("NVIDIA registry access: {0}")]
    Io(#[from] io::Error),
    #[error("ShadowPlay still reports {actual} after switching to {expected}; retry")]
    NotApplied { expected: String, actual: String },
    #[error("NVIDIA ShadowPlay: {0}")]
    #[cfg(windows)]
    Api(#[from] shadowplay::ApiError),
    #[error("this app requires Windows and the NVIDIA overlay")]
    #[cfg(not(windows))]
    Unsupported,
}

impl RawValue {
    pub(crate) fn path(&self) -> Result<String, NvidiaError> {
        // NVIDIA versions use both REG_SZ and UTF-16 REG_BINARY.
        if !matches!(self.kind, 1..=3)
            || !self.bytes.len().is_multiple_of(2)
            || self.bytes.len() > MAX_REGISTRY_VALUE_BYTES
        {
            return Err(NvidiaError::Format);
        }

        let mut words: Vec<u16> = self
            .bytes
            .as_chunks::<2>()
            .0
            .iter()
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

/// Plan the redirection from the location the running `ShadowPlay` engine uses right now.
pub(crate) fn plan(target: String) -> Result<Redirect, NvidiaError> {
    let original_path = current_path()?;
    if inside_mount(&original_path, &target) {
        return Err(NvidiaError::RamOriginal);
    }

    // Keep the registry's exact type and bytes while they agree with the live value, so the
    // registry fallback can restore them verbatim.
    let original = match read() {
        Ok(value) if value.path().is_ok_and(|path| path == original_path) => value,
        _ => RawValue {
            kind: 3,
            bytes: Vec::new(),
        }
        .with_path(&original_path),
    };

    Ok(Redirect {
        replacement: original.with_path(&target),
        original,
        original_path,
        target,
    })
}

/// Whether `path` lies on the RAM volume that `target` (`<mount>\NVIDIA-Replay`) belongs to:
/// NVIDIA pointing there without a recovery journal means the real location is unknown.
pub(crate) fn inside_mount(path: &str, target: &str) -> bool {
    let mount = target.rsplit_once('\\').map_or(target, |(mount, _)| mount);
    path.get(..mount.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(mount))
        && matches!(path[mount.len()..].chars().next(), None | Some('\\'))
}

/// Switch the running engine to the RAM location; the engine persists the value itself.
pub(crate) fn apply(redirect: &Redirect) -> Result<(), NvidiaError> {
    set_live_path(&redirect.target)?;

    let actual = current_path()?;
    if actual.eq_ignore_ascii_case(&redirect.target) {
        Ok(())
    } else {
        Err(NvidiaError::NotApplied {
            expected: redirect.target.clone(),
            actual,
        })
    }
}

/// Whether the engine still uses the RAM location; the overlay re-pushes its own stored
/// location whenever it restarts, which silently undoes the redirection.
pub(crate) fn redirected(redirect: &Redirect) -> Result<bool, NvidiaError> {
    Ok(redirect.restores(&live_path()?))
}

impl NvidiaError {
    /// The engine could not be reached at all, as opposed to refusing a value.
    pub(crate) fn is_engine_unavailable(&self) -> bool {
        #[cfg(windows)]
        {
            matches!(self, Self::Api(_))
        }
        #[cfg(not(windows))]
        {
            matches!(self, Self::Unsupported)
        }
    }
}

impl Redirect {
    /// Only this app's own value is restored; a later user or overlay edit takes precedence.
    fn restores(&self, current: &str) -> bool {
        current.eq_ignore_ascii_case(&self.target)
    }

    fn restore_value(&self, current: &RawValue) -> Option<&RawValue> {
        (current == &self.replacement).then_some(&self.original)
    }
}

/// Restore the original location in the running engine, or in the registry it reads at
/// its next start when the engine cannot be reached.
pub(crate) fn restore(redirect: &Redirect) -> Result<(), NvidiaError> {
    match live_path() {
        Ok(current) => {
            if redirect.restores(&current) {
                set_live_path(&redirect.original_path)?;
            }

            Ok(())
        }
        Err(_) => restore_registry(redirect),
    }
}

fn restore_registry(redirect: &Redirect) -> Result<(), NvidiaError> {
    match read() {
        Ok(value) => {
            if let Some(original) = redirect.restore_value(&value) {
                write(original)?;
            }

            Ok(())
        }
        Err(NvidiaError::Missing) => Ok(()),
        Err(error) => Err(error),
    }
}

/// The engine's current location, validated like a registry path.
fn current_path() -> Result<String, NvidiaError> {
    let path = live_path()?;
    RawValue {
        kind: 1,
        bytes: Vec::new(),
    }
    .with_path(&path)
    .path()
}

#[cfg(windows)]
fn live_path() -> Result<String, NvidiaError> {
    Ok(shadowplay::text(shadowplay::TEMPORARY_PATH)?)
}

#[cfg(windows)]
fn set_live_path(path: &str) -> Result<(), NvidiaError> {
    Ok(shadowplay::set_text(shadowplay::TEMPORARY_PATH, path)?)
}

#[cfg(not(windows))]
const fn live_path() -> Result<String, NvidiaError> {
    Err(NvidiaError::Unsupported)
}

#[cfg(not(windows))]
const fn set_live_path(_: &str) -> Result<(), NvidiaError> {
    Err(NvidiaError::Unsupported)
}

#[cfg(windows)]
const REGISTRY_KEY: &str = r"Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS";

#[cfg(windows)]
const TEMP_PATH_VALUE: &str = "TempFilePath";

#[cfg(windows)]
fn read() -> Result<RawValue, NvidiaError> {
    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WOW64_64KEY},
    };

    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(REGISTRY_KEY, KEY_READ | KEY_WOW64_64KEY)
        .map_err(registry_error)?;
    let value = key.get_raw_value(TEMP_PATH_VALUE).map_err(registry_error)?;

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
        .open_subkey_with_flags(REGISTRY_KEY, KEY_WRITE | KEY_WOW64_64KEY)?;
    key.set_raw_value(
        TEMP_PATH_VALUE,
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
mod tests;
