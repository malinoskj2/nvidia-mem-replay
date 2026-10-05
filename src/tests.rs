use super::*;
use crate::sys::nvidia::{RawValue, Redirect};

fn pending_redirect() -> Redirect {
    let original = RawValue {
        kind: 1,
        bytes: Vec::new(),
    }
    .with_path(r"C:\NVIDIA");
    Redirect {
        replacement: original.with_path(r"R:\Temp"),
        original,
        original_path: r"C:\NVIDIA".to_owned(),
        target: r"R:\Temp".to_owned(),
    }
}

#[test]
fn pending_recovery_precedes_malformed_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let store = storage::Store::at(directory.path().to_owned()).unwrap();
    store.save_redirect(&pending_redirect()).unwrap();
    std::fs::write(directory.path().join("config.json"), b"broken").unwrap();
    let mut restored = false;
    let result = startup_config(&store, |redirect| {
        assert_eq!(redirect.original_path, r"C:\NVIDIA");
        restored = true;
        Ok(())
    });
    assert!(restored);
    assert!(result.is_err());
    assert!(store.redirect().unwrap().is_none());
}

#[test]
fn failed_recovery_retains_journal_and_can_retry_before_loading_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let store = storage::Store::at(directory.path().to_owned()).unwrap();
    store.save_redirect(&pending_redirect()).unwrap();
    std::fs::write(directory.path().join("config.json"), b"broken").unwrap();
    let failure = startup_config(&store, |_| anyhow::bail!("registry unavailable"));
    assert!(format!("{:#}", failure.unwrap_err()).contains("registry unavailable"));
    assert!(store.redirect().unwrap().is_some());
    assert!(startup_config(&store, |_| Ok(())).is_err());
    assert!(store.redirect().unwrap().is_none());
}
