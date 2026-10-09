use super::*;

#[test]
fn migrates_legacy_helper_without_accepting_unknown_settings() {
    let config: Config =
        serde_json::from_str(r#"{"helper":"C:\\old.exe","drive":"T","memory_limit_mb":1024}"#)
            .unwrap();

    assert_eq!(config.drive, 'T');
    assert_eq!(config.memory_limit_mb, 1024);
    assert_eq!(config.placement, Placement::Hidden);
    assert!(!serde_json::to_string(&config).unwrap().contains("helper"));
    assert!(serde_json::from_str::<Config>(r#"{"memory_limit_mbb":1024}"#).is_err());
}

#[test]
fn placement_round_trips_and_defaults_to_hidden() {
    let config: Config =
        serde_json::from_str(r#"{"placement":"drive","drive":"T","memory_limit_mb":1024}"#)
            .unwrap();
    assert_eq!(config.placement, Placement::Drive);
    assert_eq!(config.mount(), "T:");
    assert_eq!(config.target(), r"T:\NVIDIA-Replay");

    let saved = serde_json::to_string(&config).unwrap();
    assert!(saved.contains(r#""placement":"drive""#));

    assert_eq!(Config::default().placement, Placement::Hidden);
    assert!(serde_json::from_str::<Config>(r#"{"placement":"floppy"}"#).is_err());
}

#[test]
fn hidden_placement_mounts_inside_the_state_directory() {
    let config = Config::default();
    let state = crate::storage::state_directory().display().to_string();

    assert_eq!(config.mount(), format!("{state}\\ram"));
    assert_eq!(config.target(), format!("{state}\\ram\\NVIDIA-Replay"));
    assert_eq!(config.volume().limit_bytes, 8_192_000_000);
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
