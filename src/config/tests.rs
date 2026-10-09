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
    let state = crate::storage::state_directory().display().to_string();

    assert_eq!(Config::mount(), format!("{state}\\ram"));
    assert_eq!(config.target(), format!("{state}\\ram\\NVIDIA-Replay"));
    assert_eq!(config.volume().limit_bytes, 8_192_000_000);
    assert_eq!(
        Volume::new("T:".to_owned(), 256).target(),
        r"T:\NVIDIA-Replay"
    );
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
