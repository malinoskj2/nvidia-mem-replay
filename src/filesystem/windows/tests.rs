use super::*;

// The mount-point rules themselves are covered in `config::tests`; this checks the wiring.

#[test]
fn filesystem_cli_accepts_drive_letters_and_absolute_directories() {
    for mount in ["T:", r"C:\Users\me\AppData\Local\NvidiaMemReplay\ram"] {
        assert!(
            Cli::try_parse_from([
                "replay",
                "filesystem",
                "--mount",
                mount,
                "--memory-limit-mb",
                "256"
            ])
            .is_ok(),
            "{mount}"
        );
    }
}

#[test]
fn filesystem_cli_rejects_invalid_or_incomplete_configuration() {
    for args in [
        vec!["replay", "filesystem"],
        vec![
            "replay",
            "filesystem",
            "--mount",
            "C:",
            "--memory-limit-mb",
            "256",
        ],
        vec![
            "replay",
            "filesystem",
            "--mount",
            r"C:\ram\",
            "--memory-limit-mb",
            "256",
        ],
        vec![
            "replay",
            "filesystem",
            "--mount",
            "T:",
            "--memory-limit-mb",
            "0",
        ],
        vec![
            "replay",
            "filesystem",
            "--mount",
            "T:",
            "--memory-limit-mb",
            "65537",
        ],
    ] {
        assert!(Cli::try_parse_from(args.clone()).is_err(), "{args:?}");
    }
}
