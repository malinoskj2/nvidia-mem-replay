use std::fmt::Display;

pub(super) const STOP_AND_RESTORE: &str = "Stop and restore";
pub(super) const RETRY_START: &str = "Retry / start";
pub(super) const SETTINGS: &str = "Settings";
pub(super) const HIDE_TO_TRAY: &str = "Hide to tray";
pub(super) const QUIT: &str = "Quit";
pub(super) const RETRY_SHUTDOWN: &str = "Retry shutdown";
pub(super) const EXIT_ANYWAY: &str = "Exit anyway";
pub(super) const SETTINGS_TITLE: &str = "RAM storage settings";
pub(super) const APPLY_AND_RESTART: &str = "Apply and restart";
pub(super) const LIFETIME_WRITES: &str = "Written · lifetime";
pub(super) const ALLOCATED_BUFFER: &str = "Buffer allocated";
pub(super) const DRIVE: &str = "Drive";
pub(super) const MEMORY_CEILING: &str = "Ceiling (MB)";

pub(super) const SUBTITLE: &str = "NVIDIA Instant Replay temporary storage";
pub(super) const CLOSE_WITH_TRAY: &str =
    "Closing hides to tray. Quit restores the path and discards the buffer.";
pub(super) const CLOSE_WITHOUT_TRAY: &str =
    "Closing restores the path and discards the RAM buffer.";
pub(super) const SHUTDOWN_PENDING: &str = "Restoring the temporary path and stopping RAM storage…";
pub(super) const SHUTDOWN_FAILED_HELP: &str = "Resolve the error and retry. If restoration failed, the recovery journal is kept for the next launch; you can also set Temporary files to a persistent drive in Alt+Z.";
pub(super) const RESTART_NOTICE: &str =
    "Save any wanted replay first; restarting discards the current buffer.";
pub(super) const LOW_MEMORY_WARNING: &str =
    "System memory is low. Reduce replay length or bitrate.";
pub(super) const BUFFER_LIMIT_WARNING: &str =
    "Near the memory ceiling. Recording may stop if the buffer fills.";
pub(super) const RECORDING_HELP: &str = "NVIDIA uses the RAM location from the next time Instant Replay is switched on: toggle it off/on in Alt+Z. Keep Gallery on a persistent drive.";
pub(super) const STARTUP_FAILED_TITLE: &str = "nvidia-mem-replay could not start";
pub(super) const STARTUP_FAILED_HELP: &str =
    "Close this window, resolve the error, then launch again.";

pub(super) fn tray_unavailable(error: &impl Display) -> String {
    format!("Tray unavailable; closing will quit: {error}")
}

pub(super) fn status(message: &str) -> String {
    format!("●  {message}")
}

pub(super) fn lifetime_written(bytes: u64) -> String {
    format!("{:.3} GB", bytes as f64 / 1e9)
}

pub(super) fn buffer_allocated(bytes: Option<u64>) -> String {
    bytes.map_or_else(
        || "— MB".to_owned(),
        |bytes| format!("{:.1} MB", bytes as f64 / 1e6),
    )
}

pub(super) fn memory_summary(resident_bytes: Option<u64>, available_bytes: u64) -> String {
    let resident = resident_bytes.map_or_else(
        || "unavailable".to_owned(),
        |bytes| format!("{:.1} MB", bytes as f64 / 1e6),
    );

    format!(
        "Filesystem process RAM: {resident} · System available: {:.0} MB",
        available_bytes as f64 / 1e6
    )
}

pub(super) fn original_location(path: &str) -> String {
    format!("Detected: {path}")
}

pub(super) fn ram_location(path: &str) -> String {
    format!("RAM temporary files: {path}")
}

pub(super) fn buffer_ceiling(bytes: u64) -> String {
    format!("Buffer ceiling: {} MB", bytes / 1_000_000)
}

pub(super) fn drive(letter: char) -> String {
    format!("{letter}:")
}
