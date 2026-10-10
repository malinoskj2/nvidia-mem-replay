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
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_settings_without_accepting_unknown_ones() {
        let config: Config = serde_json::from_str(
            r#"{"helper":"C:\\old.exe","placement":"drive","drive":"T","memory_limit_mb":1024}"#,
        )
        .unwrap();

        assert_eq!(config.memory_limit_mb, 1024);
        let saved = serde_json::to_string(&config).unwrap();
        assert_eq!(saved, r#"{"memory_limit_mb":1024}"#);
        assert!(serde_json::from_str::<Config>(r#"{"memory_limit_mbb":1024}"#).is_err());
        assert_eq!(
            serde_json::from_str::<Config>("{}")
                .unwrap()
                .memory_limit_mb,
            8192
        );
    }

    #[test]
    fn volume_is_mounted_inside_the_state_directory() {
        let config = Config::default();
        let mount = crate::storage::state_directory()
            .join("ram")
            .display()
            .to_string();

        assert_eq!(Config::mount(), mount);
        assert_eq!(config.target(), format!("{mount}\\NVIDIA-Replay"));
        assert_eq!(config.volume().limit_bytes, 8_192_000_000);
        assert_eq!(
            Volume::new("T:".to_owned(), 256).target(),
            r"T:\NVIDIA-Replay"
        );
    }

    #[test]
    fn mount_points_are_drive_letters_or_absolute_backslash_directories() {
        for mount in [
            "D:",
            "T:",
            "Z:",
            r"C:\ram",
            r"c:\ram",
            r"D:\ram",
            r"C:\Users\me\AppData\Local\NvidiaMemReplay\ram",
            r"C:\Users\me\.hidden\...\ram",
            r"C:\影像\ram",
        ] {
            assert_eq!(mount_point(mount), Ok(mount));
        }
        assert!(is_drive_letter("T:"));
        assert!(!is_drive_letter("C:"));
        assert!(!is_drive_letter(r"T:\"));
        assert!(!is_drive_letter(r"D:\ram"));

        let longest = format!(r"C:\{}", "x".repeat(MAX_MOUNT_POINT_LENGTH - 3));
        assert!(mount_point(&longest).is_ok());
        assert!(mount_point(&format!("{longest}x")).is_err());
    }

    #[test]
    fn malformed_mount_points_are_rejected_like_the_native_helper_does() {
        for mount in [
            "",
            "C:",
            "A:",
            "c:",
            "T",
            "TT:",
            "T;",
            "1:",
            r"T:\",
            r"C:\ram\",
            r"C:\\ram",
            r"C:\ram\\here",
            r"C:ram",
            r"C:/ram",
            r"C:\ram/here",
            r"C:\.",
            r"C:\..",
            r"C:\ram\.",
            r"C:\.\ram",
            r"C:\ram\..\x",
            r"ram\here",
            r"\ram",
            r"\\server\share",
            r"\\?\C:\ram",
            r"\\.\C:\ram",
            r"1:\ram",
            r"é:\ram",
        ] {
            let rejected = mount_point(mount);
            assert!(rejected.is_err(), "{mount}");
            assert!(rejected.unwrap_err().contains("drive letter"));
        }
    }

    #[test]
    fn rejects_unsafe_configuration() {
        let mut config = Config::default();
        assert!(config.validate().is_ok());

        config.memory_limit_mb = 0;
        assert!(matches!(config.validate(), Err(ConfigError::Memory)));
        config.memory_limit_mb = MAX_MEMORY_LIMIT_MB + 1;
        assert!(matches!(config.validate(), Err(ConfigError::Memory)));
    }
}
