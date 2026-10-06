use serde::{Deserialize, Serialize};
use thiserror::Error;

pub(crate) const MIN_DRIVE: char = 'D';
pub(crate) const MAX_DRIVE: char = 'Z';
pub(crate) const MIN_MEMORY_LIMIT_MB: u32 = 256;
pub(crate) const MAX_MEMORY_LIMIT_MB: u32 = 65_536;
const RECORDING_DIRECTORY: &str = "NVIDIA-Replay";

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Config {
    pub(crate) drive: char,
    pub(crate) memory_limit_mb: u32,
}

// Accept only the known legacy helper field while rejecting unrelated misspellings.
impl<'de> Deserialize<'de> for Config {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct Saved {
            drive: char,
            memory_limit_mb: u32,
            #[serde(rename = "helper")]
            _legacy_helper: Option<serde::de::IgnoredAny>,
        }

        impl Default for Saved {
            fn default() -> Self {
                let config = Config::default();

                Self {
                    drive: config.drive,
                    memory_limit_mb: config.memory_limit_mb,
                    _legacy_helper: None,
                }
            }
        }

        let saved = Saved::deserialize(deserializer)?;

        Ok(Self {
            drive: saved.drive,
            memory_limit_mb: saved.memory_limit_mb,
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
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
        format!("{}:", self.drive)
    }

    pub(crate) fn target(&self) -> String {
        format!("{}:\\{RECORDING_DIRECTORY}", self.drive)
    }

    pub(crate) const fn limit_bytes(&self) -> u64 {
        self.memory_limit_mb as u64 * 1_000_000
    }
}

#[cfg(test)]
mod tests;
