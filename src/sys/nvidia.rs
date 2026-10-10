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
    /// The engine could not be reached when the live location was first read; the NVIDIA App
    /// is not running (yet).
    #[error("the NVIDIA App is not running or its ShadowPlay engine could not be reached: {0}")]
    #[cfg(windows)]
    Unreachable(#[source] shadowplay::ApiError),
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
/// This is the first call of a start, so an engine that cannot be reached here is reported as
/// [`NvidiaError::Unreachable`]; later calls report their failures as they are.
pub(crate) fn plan(target: String) -> Result<Redirect, NvidiaError> {
    let original_path = current_path().map_err(NvidiaError::while_reaching)?;
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
    /// The engine could not be reached at all, as opposed to refusing a value or failing
    /// internally; the caller may simply wait for the NVIDIA App to start.
    pub(crate) const fn is_engine_unavailable(&self) -> bool {
        #[cfg(windows)]
        {
            matches!(self, Self::Unreachable(_))
        }
        #[cfg(not(windows))]
        {
            matches!(self, Self::Unsupported)
        }
    }

    /// Reinterpret a failure of the first engine call: an API failure to connect means the
    /// engine is not there, while value and internal errors stay what they are.
    #[cfg(windows)]
    fn while_reaching(self) -> Self {
        match self {
            Self::Api(error) if error.is_connection_failure() => Self::Unreachable(error),
            other => other,
        }
    }

    /// Without an engine to reach, every error already says what it is.
    #[cfg(not(windows))]
    const fn while_reaching(self) -> Self {
        self
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
mod tests {
    use super::*;

    #[test]
    fn only_paths_on_the_ram_volume_count_as_inside_the_mount() {
        let target = r"C:\Users\me\AppData\Local\NvidiaMemReplay\ram\NVIDIA-Replay";
        assert!(inside_mount(target, target));
        assert!(inside_mount(
            r"c:\users\ME\appdata\local\nvidiamemreplay\RAM",
            target
        ));
        assert!(inside_mount(
            r"C:\Users\me\AppData\Local\NvidiaMemReplay\ram\other",
            target
        ));
        assert!(!inside_mount(r"C:\Users\me\Videos\temp", target));
        assert!(!inside_mount(
            r"C:\Users\me\AppData\Local\NvidiaMemReplay\ramdisk",
            target
        ));
        assert!(!inside_mount(r"C:\Users\me\AppData\Local", target));

        assert!(inside_mount(r"T:\anything", r"T:\NVIDIA-Replay"));
        assert!(!inside_mount(r"S:\NvidiaTemp", r"T:\NVIDIA-Replay"));
    }

    #[cfg(windows)]
    #[test]
    fn only_connection_failures_of_the_first_call_make_the_engine_unavailable() {
        use shadowplay::ApiError;

        let refused = || {
            NvidiaError::Api(ApiError::Call {
                name: shadowplay::TEMPORARY_PATH.to_owned(),
                result: -1,
            })
        };

        // Reading the live location first: a failed connection means the engine is not there.
        let unreachable = refused().while_reaching();
        assert!(matches!(unreachable, NvidiaError::Unreachable(_)));
        assert!(unreachable.is_engine_unavailable());
        assert!(
            unreachable
                .to_string()
                .contains("NVIDIA App is not running")
        );
        assert!(unreachable.to_string().contains("TempFilePath"));
        assert!(
            NvidiaError::Api(ApiError::Create(-1))
                .while_reaching()
                .is_engine_unavailable()
        );

        // The same HRESULT from a later call (apply, restore, the watchdog) is a real error.
        assert!(!refused().is_engine_unavailable());

        // Internal and value errors never count as an unreachable engine, whenever they happen.
        for error in [
            NvidiaError::Api(ApiError::Name),
            NvidiaError::Api(ApiError::Memory),
            NvidiaError::Api(ApiError::Lock),
            NvidiaError::Api(ApiError::NotFound),
            NvidiaError::Api(ApiError::NoText(shadowplay::TEMPORARY_PATH.to_owned())),
            NvidiaError::Path,
            NvidiaError::Format,
            NvidiaError::RamOriginal,
        ] {
            let message = error.to_string();
            let reinterpreted = error.while_reaching();
            assert!(!reinterpreted.is_engine_unavailable(), "{message}");
            assert_eq!(reinterpreted.to_string(), message);
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn only_the_missing_platform_support_makes_the_engine_unavailable() {
        assert!(NvidiaError::Unsupported.is_engine_unavailable());
        assert!(
            NvidiaError::Unsupported
                .while_reaching()
                .is_engine_unavailable()
        );
        for error in [
            NvidiaError::Path,
            NvidiaError::Format,
            NvidiaError::RamOriginal,
        ] {
            assert!(!error.while_reaching().is_engine_unavailable());
        }
    }

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

    #[test]
    fn live_restoration_only_replaces_this_apps_own_value() {
        let redirect = Redirect {
            original: RawValue {
                kind: 3,
                bytes: vec![],
            }
            .with_path(r"C:\Temp"),
            replacement: RawValue {
                kind: 3,
                bytes: vec![],
            }
            .with_path(r"R:\NVIDIA-Replay"),
            original_path: r"C:\Temp".to_owned(),
            target: r"R:\NVIDIA-Replay".to_owned(),
        };

        assert!(redirect.restores(r"R:\NVIDIA-Replay"));
        assert!(redirect.restores(r"r:\nvidia-replay"));
        assert!(!redirect.restores(r"D:\NewTemp"));
        assert!(!redirect.restores(r"C:\Temp"));
    }
}
