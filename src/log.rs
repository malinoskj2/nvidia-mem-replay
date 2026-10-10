//! What the app did and what went wrong: kept in memory for the Logs tab and mirrored to a
//! file in the state directory, so a problem can be read back or attached to a bug report.

use std::{collections::VecDeque, fmt::Display, fs::File, io::Write, path::Path, sync::Mutex};

/// Entries kept in memory; the file keeps everything from the session.
const CAPACITY: usize = 500;
pub(crate) const FILE_NAME: &str = "nvidia-mem-replay.log";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Level {
    Info,
    Warning,
    Error,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    /// Increases by one per entry; lets a reader fetch only what it has not seen.
    pub(crate) sequence: u64,
    /// Wall-clock time, `HH:MM:SS`: local time on Windows, UTC elsewhere.
    pub(crate) time: String,
    pub(crate) level: Level,
    pub(crate) message: String,
}

impl Entry {
    /// One line: time, level and message.
    pub(crate) fn line(&self) -> String {
        format!("{}  {:<7}  {}", self.time, self.level.tag(), self.message)
    }
}

struct Journal {
    entries: VecDeque<Entry>,
    next_sequence: u64,
}

static JOURNAL: Mutex<Journal> = Mutex::new(Journal {
    entries: VecDeque::new(),
    next_sequence: 1,
});

/// The mirror file, apart from the journal so that readers never wait on the disk. A writer
/// takes this lock first and the journal lock inside it, which keeps the file in sequence order.
static FILE: Mutex<Option<File>> = Mutex::new(None);

pub(crate) fn info(message: impl Display) {
    record(Level::Info, message);
}

pub(crate) fn warning(message: impl Display) {
    record(Level::Warning, message);
}

pub(crate) fn error(message: impl Display) {
    record(Level::Error, message);
}

/// Entries recorded after `sequence` (0 for all that are still in memory), oldest first.
pub(crate) fn since(sequence: u64) -> Vec<Entry> {
    lock()
        .entries
        .iter()
        .filter(|entry| entry.sequence > sequence)
        .cloned()
        .collect()
}

/// Writes the entries so far to `path`, replacing the file from an earlier run, and every
/// later entry as it is recorded.
pub(crate) fn mirror_to(path: &Path) -> std::io::Result<()> {
    let mut created = File::create(path)?;
    let mut file = file_lock();
    // No entry is recorded while the file lock is held, so nothing falls between the copy and
    // the switch to the new file.
    let lines: Vec<String> = lock().entries.iter().map(Entry::line).collect();
    for line in lines {
        writeln!(created, "{line}")?;
    }
    *file = Some(created);
    Ok(())
}

/// Stops mirroring and hands back the file, so a test can let its directory go.
#[cfg(test)]
fn detach_file() -> Option<File> {
    file_lock().take()
}

fn record(level: Level, message: impl Display) {
    let time = timestamp();
    let message = message.to_string();
    let mut file = file_lock();
    let line = {
        let mut journal = lock();
        let entry = Entry {
            sequence: journal.next_sequence,
            time,
            level,
            message,
        };
        let line = entry.line();
        journal.next_sequence += 1;
        journal.entries.push_back(entry);
        if journal.entries.len() > CAPACITY {
            journal.entries.pop_front();
        }
        line
    };
    if let Some(file) = file.as_mut() {
        // A full disk must not take the application down with it.
        let _ = writeln!(file, "{line}");
    }
}

fn lock() -> std::sync::MutexGuard<'static, Journal> {
    JOURNAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn file_lock() -> std::sync::MutexGuard<'static, Option<File>> {
    FILE.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The local date, `YYYY-MM-DD`.
#[cfg(windows)]
pub(crate) fn today() -> String {
    let now = winsafe::GetLocalTime();
    format!("{:04}-{:02}-{:02}", now.wYear, now.wMonth, now.wDay)
}

#[cfg(windows)]
fn timestamp() -> String {
    let now = winsafe::GetLocalTime();
    format!("{:02}:{:02}:{:02}", now.wHour, now.wMinute, now.wSecond)
}

#[cfg(not(windows))]
fn timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
        % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
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
}
