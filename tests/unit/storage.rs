use super::*;

#[test]
fn state_round_trips_and_lock_excludes_second_instance() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    assert!(matches!(
        Store::at(directory.path().to_owned()),
        Err(StorageError::Locked)
    ));
    assert_eq!(store.lifetime().unwrap(), 0);
    store.save_lifetime(123).unwrap();
    store.save_lifetime(456).unwrap();
    assert_eq!(store.lifetime().unwrap(), 456);
    drop(store);
    assert_eq!(
        Store::at(directory.path().to_owned())
            .unwrap()
            .lifetime()
            .unwrap(),
        456
    );
}

#[test]
fn corrupt_state_is_not_silently_reset() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    fs::write(directory.path().join("lifetime.json"), b"broken").unwrap();
    assert!(matches!(store.lifetime(), Err(StorageError::Json(_))));
}

#[test]
fn oversized_state_is_rejected_before_persistence() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().to_owned()).unwrap();
    assert!(matches!(
        store.write("large.json", &"x".repeat(65_537)),
        Err(StorageError::Oversized)
    ));
    assert!(!directory.path().join("large.json").exists());
}
