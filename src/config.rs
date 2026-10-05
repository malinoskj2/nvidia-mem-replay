use serde::{Deserialize, Serialize};
use thiserror::Error;

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
            memory_limit_mb: 4096,
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
        if !('D'..='Z').contains(&self.drive) {
            return Err(ConfigError::Drive);
        }
        if !(256..=65536).contains(&self.memory_limit_mb) {
            return Err(ConfigError::Memory);
        }
        Ok(())
    }

    pub(crate) fn mount(&self) -> String {
        format!("{}:", self.drive)
    }

    pub(crate) fn target(&self) -> String {
        format!("{}:\\NVIDIA-Replay", self.drive)
    }

    pub(crate) const fn limit_bytes(&self) -> u64 {
        self.memory_limit_mb as u64 * 1_000_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_helper_without_accepting_unknown_settings() {
        let config: Config =
            serde_json::from_str(r#"{"helper":"C:\\old.exe","drive":"T","memory_limit_mb":1024}"#)
                .unwrap();
        assert_eq!(config.drive, 'T');
        assert_eq!(config.memory_limit_mb, 1024);
        assert!(!serde_json::to_string(&config).unwrap().contains("helper"));
        assert!(serde_json::from_str::<Config>(r#"{"memory_limit_mbb":1024}"#).is_err());
    }

    #[test]
    fn rejects_unsafe_configuration() {
        let mut config = Config::default();
        assert!(config.validate().is_ok());
        config.drive = 'C';
        assert!(matches!(config.validate(), Err(ConfigError::Drive)));
        config.drive = 'R';
        config.memory_limit_mb = 0;
        assert!(matches!(config.validate(), Err(ConfigError::Memory)));
    }
}
