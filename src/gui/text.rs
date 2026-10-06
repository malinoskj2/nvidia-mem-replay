use std::fmt::Display;

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
pub(super) const RECORDING_HELP: &str = "If writes do not start, toggle Instant Replay off/on in Alt+Z. Keep Gallery on a persistent drive.";
pub(super) const STARTUP_FAILED_TITLE: &str = "Replay in RAM could not start";
pub(super) const STARTUP_FAILED_HELP: &str =
    "Close this window, resolve the error, then launch again.";

pub(super) fn tray_unavailable(error: &impl Display) -> String {
    format!("Tray unavailable; closing will quit: {error}")
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
