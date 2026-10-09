use serde::{Deserialize, Serialize};
use thiserror::Error;

pub(crate) const MIN_MEMORY_LIMIT_MB: u32 = 256;
pub(crate) const MAX_MEMORY_LIMIT_MB: u32 = 65_536;
const RECORDING_DIRECTORY: &str = "NVIDIA-Replay";
/// The RAM volume is mounted at this directory inside the application state folder, so it has
/// no drive letter and never shows up in Explorer or file dialogs.
const MOUNT_DIRECTORY: &str = "ram";

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Config {
    pub(crate) memory_limit_mb: u32,
}

// Accept the settings of earlier versions (a custom helper path, a drive letter and its
// placement) while rejecting unrelated misspellings.
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

/// A RAM volume as the helper processes see it: its mount point and size ceiling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Volume {
    /// `X:` for a drive letter, otherwise the absolute directory the volume is mounted at.
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

    /// The directory NVIDIA records into.
    pub(crate) fn target(&self) -> String {
        format!("{}\\{RECORDING_DIRECTORY}", self.mount)
    }
}

pub(crate) const fn limit_bytes(memory_limit_mb: u32) -> u64 {
    memory_limit_mb as u64 * 1_000_000
}

impl Config {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if !(MIN_MEMORY_LIMIT_MB..=MAX_MEMORY_LIMIT_MB).contains(&self.memory_limit_mb) {
            return Err(ConfigError::Memory);
        }
        Ok(())
    }

    /// The mount point every configuration uses.
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
