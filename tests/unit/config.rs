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
