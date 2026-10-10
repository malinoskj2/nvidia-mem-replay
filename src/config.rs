use serde::{Deserialize, Serialize};
use thiserror::Error;

pub(crate) const MIN_MEMORY_LIMIT_MB: u32 = 256;
pub(crate) const MAX_MEMORY_LIMIT_MB: u32 = 65_536;
const RECORDING_DIRECTORY: &str = "NVIDIA-Replay";
const MOUNT_DIRECTORY: &str = "ram";
pub(crate) const MIN_DRIVE: char = 'D';
pub(crate) const MAX_DRIVE: char = 'Z';
const MAX_MOUNT_POINT_LENGTH: usize = 4096;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Config {
    pub(crate) memory_limit_mb: u32,
}

impl<'de> Deserialize<'de> for Config {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct Saved {
            memory_limit_mb: u32,
            #[serde(rename = "helper")]
            _legacy_helper: Option<serde::de::IgnoredAny>,
            #[serde(rename = "drive")]
            _legacy_drive: Option<serde::de::IgnoredAny>,
            #[serde(rename = "placement")]
            _legacy_placement: Option<serde::de::IgnoredAny>,
        }

        impl Default for Saved {
            fn default() -> Self {
                Self {
                    memory_limit_mb: Config::default().memory_limit_mb,
                    _legacy_helper: None,
                    _legacy_drive: None,
                    _legacy_placement: None,
                }
            }
        }

        let saved = Saved::deserialize(deserializer)?;

        Ok(Self {
            memory_limit_mb: saved.memory_limit_mb,
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            memory_limit_mb: 8192,
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum ConfigError {
    #[error("memory ceiling must be between 256 and 65536 MB")]
    Memory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Volume {
    pub(crate) mount: String,
    pub(crate) limit_bytes: u64,
}

impl Volume {
    pub(crate) fn new(mount: String, memory_limit_mb: u32) -> Self {
        Self {
            mount,
            limit_bytes: limit_bytes(memory_limit_mb),
        }
    }

    pub(crate) fn target(&self) -> String {
        format!("{}\\{RECORDING_DIRECTORY}", self.mount)
    }
}

pub(crate) const fn limit_bytes(memory_limit_mb: u32) -> u64 {
    memory_limit_mb as u64 * 1_000_000
}

pub(crate) fn is_drive_letter(mount: &str) -> bool {
    let mut chars = mount.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(letter), Some(':'), None) if (MIN_DRIVE..=MAX_DRIVE).contains(&letter)
    )
}

pub(crate) fn mount_point(value: &str) -> Result<&str, String> {
    if is_drive_letter(value) || is_mount_directory(value) {
        Ok(value)
    } else {
        Err(format!(
            "mount point must be a drive letter {MIN_DRIVE}: through {MAX_DRIVE}: or an absolute directory such as C:\\ram"
        ))
    }
}

fn is_mount_directory(value: &str) -> bool {
    let mut chars = value.chars();
    let on_drive = matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(letter), Some(':'), Some('\\')) if letter.is_ascii_alphabetic()
    );
    let steps = chars.as_str();

    on_drive
        && !steps.is_empty()
        && value.encode_utf16().count() <= MAX_MOUNT_POINT_LENGTH
        && !value.contains('/')
        && steps
            .split('\\')
            .all(|step| !matches!(step, "" | "." | ".."))
}

impl Config {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if !(MIN_MEMORY_LIMIT_MB..=MAX_MEMORY_LIMIT_MB).contains(&self.memory_limit_mb) {
            return Err(ConfigError::Memory);
        }
        Ok(())
    }

    pub(crate) fn mount() -> String {
        crate::storage::state_directory()
            .join(MOUNT_DIRECTORY)
            .display()
            .to_string()
    }

    pub(crate) fn volume(&self) -> Volume {
        Volume::new(Self::mount(), self.memory_limit_mb)
    }

    pub(crate) fn target(&self) -> String {
        self.volume().target()
    }

    pub(crate) const fn limit_bytes(&self) -> u64 {
        limit_bytes(self.memory_limit_mb)
    }
}

#[cfg(test)]
mod tests;
