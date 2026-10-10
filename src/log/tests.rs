use super::*;

// The journal is process-wide, so every test reasons relative to its own entries.

#[test]
fn entries_are_sequenced_and_formatted_as_lines() {
    info("log test: first");
    let first = since(0)
        .into_iter()
        .rev()
        .find(|entry| entry.message == "log test: first")
        .unwrap();
    assert_eq!(first.level, Level::Info);
    assert_eq!(first.time.len(), 8);
    assert_eq!(&first.line()[8..], "  info     log test: first");

    error("log test: second");
    let later = since(first.sequence);
    assert!(later.iter().all(|entry| entry.sequence > first.sequence));
    let second = later
        .iter()
        .find(|entry| entry.message == "log test: second")
        .unwrap();
    assert_eq!(second.level, Level::Error);
    assert_eq!(&second.line()[8..], "  error    log test: second");
    assert!(
        since(second.sequence)
            .iter()
            .all(|entry| entry.sequence > second.sequence)
    );
}

#[test]
fn memory_keeps_only_the_most_recent_entries() {
    for index in 0..CAPACITY + 10 {
        warning(format!("log test: fill {index}"));
    }
    let kept = since(0);
    assert!(kept.len() <= CAPACITY);
    assert!(
        kept.iter()
            .any(|entry| entry.message == format!("log test: fill {}", CAPACITY + 9))
    );
    assert!(!kept.iter().any(|entry| entry.message == "log test: fill 0"));
}

#[test]
fn mirrors_earlier_and_later_entries_to_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(FILE_NAME);
    info("log test: before mirror");
    mirror_to(&path).unwrap();
    info("log test: after mirror");

    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.contains("info     log test: before mirror\n"));
    assert!(written.contains("info     log test: after mirror\n"));

    // Windows cannot delete the directory while the journal still holds the file open.
    assert!(detach_file().is_some());
    directory.close().unwrap();
}
