use serde::{Deserialize, Serialize};
use thiserror::Error;

pub(crate) const MIN_DRIVE: char = 'D';
pub(crate) const MAX_DRIVE: char = 'Z';
pub(crate) const MIN_MEMORY_LIMIT_MB: u32 = 256;
pub(crate) const MAX_MEMORY_LIMIT_MB: u32 = 65_536;
const RECORDING_DIRECTORY: &str = "NVIDIA-Replay";
/// The hidden RAM volume is mounted at this directory inside the application state folder.
const HIDDEN_MOUNT_DIRECTORY: &str = "ram";

/// Where the RAM volume appears on the system.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Placement {
    /// A directory mount point inside the hidden application state folder: no drive letter,
    /// nothing in Explorer or file dialogs.
    #[default]
    Hidden,
    /// A drive letter, visible in Explorer like any removable drive.
    Drive,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Config {
    pub(crate) placement: Placement,
    /// The letter used when `placement` is [`Placement::Drive`].
    pub(crate) drive: char,
    pub(crate) memory_limit_mb: u32,
}

// Accept only the known legacy helper field while rejecting unrelated misspellings.
impl<'de> Deserialize<'de> for Config {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct Saved {
            placement: Placement,
            drive: char,
            memory_limit_mb: u32,
            #[serde(rename = "helper")]
            _legacy_helper: Option<serde::de::IgnoredAny>,
        }

        impl Default for Saved {
            fn default() -> Self {
                let config = Config::default();

                Self {
                    placement: config.placement,
                    drive: config.drive,
                    memory_limit_mb: config.memory_limit_mb,
                    _legacy_helper: None,
                }
            }
        }

        let saved = Saved::deserialize(deserializer)?;

        Ok(Self {
            placement: saved.placement,
            drive: saved.drive,
            memory_limit_mb: saved.memory_limit_mb,
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            placement: Placement::Hidden,
            drive: 'R',
            memory_limit_mb: 8192,
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum ConfigError {
    #[error("choose a drive letter from D through Z")]
    Drive,
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
        if !(MIN_DRIVE..=MAX_DRIVE).contains(&self.drive) {
            return Err(ConfigError::Drive);
        }
        if !(MIN_MEMORY_LIMIT_MB..=MAX_MEMORY_LIMIT_MB).contains(&self.memory_limit_mb) {
            return Err(ConfigError::Memory);
        }
        Ok(())
    }

    pub(crate) fn mount(&self) -> String {
        match self.placement {
            Placement::Drive => format!("{}:", self.drive),
            Placement::Hidden => crate::storage::state_directory()
                .join(HIDDEN_MOUNT_DIRECTORY)
                .display()
                .to_string(),
        }
    }

    pub(crate) fn volume(&self) -> Volume {
        Volume::new(self.mount(), self.memory_limit_mb)
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
