use super::*;

#[test]
fn filesystem_cli_rejects_invalid_or_incomplete_configuration() {
    assert!(
        Cli::try_parse_from([
            "replay",
            "filesystem",
            "--drive",
            "T",
            "--memory-limit-mb",
            "256"
        ])
        .is_ok()
    );
    for args in [
        vec!["replay", "filesystem"],
        vec![
            "replay",
            "filesystem",
            "--drive",
            "C",
            "--memory-limit-mb",
            "256",
        ],
        vec![
            "replay",
            "filesystem",
            "--drive",
            "TT",
            "--memory-limit-mb",
            "256",
        ],
        vec![
            "replay",
            "filesystem",
            "--drive",
            "T",
            "--memory-limit-mb",
            "0",
        ],
        vec![
            "replay",
            "filesystem",
            "--drive",
            "T",
            "--memory-limit-mb",
            "65537",
        ],
    ] {
        assert!(Cli::try_parse_from(args).is_err());
    }
}
