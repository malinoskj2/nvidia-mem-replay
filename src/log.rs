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
    /// Local wall-clock time, `HH:MM:SS`.
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
    file: Option<File>,
}

static JOURNAL: Mutex<Journal> = Mutex::new(Journal {
    entries: VecDeque::new(),
    next_sequence: 1,
    file: None,
});

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
    let mut file = File::create(path)?;
    let mut journal = lock();
    for entry in &journal.entries {
        writeln!(file, "{}", entry.line())?;
    }
    journal.file = Some(file);
    Ok(())
}

fn record(level: Level, message: impl Display) {
    let time = timestamp();
    let mut journal = lock();
    let entry = Entry {
        sequence: journal.next_sequence,
        time,
        level,
        message: message.to_string(),
    };
    journal.next_sequence += 1;
    if let Some(file) = &mut journal.file {
        // A full disk must not take the application down with it.
        let _ = writeln!(file, "{}", entry.line());
    }
    journal.entries.push_back(entry);
    if journal.entries.len() > CAPACITY {
        journal.entries.pop_front();
    }
}

fn lock() -> std::sync::MutexGuard<'static, Journal> {
    JOURNAL
        .lock()
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
mod tests;
