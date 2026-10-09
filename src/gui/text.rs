use std::fmt::Display;

pub(super) const TAB_STATUS: &str = "Status";
pub(super) const TAB_SETTINGS: &str = "Settings";
pub(super) const TAB_LOGS: &str = "Logs";

pub(super) const STOP_AND_RESTORE: &str = "Stop and restore";
pub(super) const RETRY_START: &str = "Start";
pub(super) const RETRY_SHUTDOWN: &str = "Retry shutdown";
pub(super) const EXIT_ANYWAY: &str = "Exit anyway";
pub(super) const APPLY_AND_RESTART: &str = "Apply and restart";
pub(super) const LIFETIME_WRITES: &str = "Written (lifetime)";
pub(super) const ALLOCATED_BUFFER: &str = "Buffer allocated";
pub(super) const MEMORY_CEILING: &str = "Ceiling (MB)";
pub(super) const CEILING_REQUIRED: &str = "Enter the memory ceiling in MB.";

pub(super) const GROUP_STATUS: &str = "Status";
pub(super) const GROUP_LOCATIONS: &str = "Temporary files";
pub(super) const GROUP_STORAGE: &str = "RAM storage";
pub(super) const GROUP_RECORDING: &str = "Recording";
pub(super) const GROUP_STARTUP: &str = "Startup";
pub(super) const START_WITH_WINDOWS: &str = "Start with Windows";
pub(super) const START_WITH_WINDOWS_HELP: &str = "Starts hidden in the notification area at sign-in and waits for the NVIDIA App before redirecting.";
pub(super) const ORIGINAL_LOCATION: &str = "NVIDIA location";
pub(super) const RAM_LOCATION: &str = "RAM location";
pub(super) const BUFFER_CEILING: &str = "Buffer ceiling";
pub(super) const PATH_SWAPPED: &str = "NVIDIA temporary files are redirected to RAM";
pub(super) const PATH_ORIGINAL: &str = "NVIDIA temporary files are at their original location";

pub(super) const STOP_HELP: &str =
    "Stops recording to RAM, restores NVIDIA's original location and discards the buffer.";
pub(super) const START_HELP: &str =
    "Mounts the RAM storage and redirects NVIDIA's temporary files to it.";
pub(super) const CLOSE_WITH_TRAY: &str =
    "Closing this window keeps recording in the tray. Right-click the tray icon to quit.";
pub(super) const SHUTDOWN_PENDING: &str = "Restoring the temporary path and stopping RAM storage…";
pub(super) const SHUTDOWN_FAILED_HELP: &str =
    "Could not finish shutdown. Retry, or exit anyway and restore Temporary files in Alt+Z.";
pub(super) const RESTART_NOTICE: &str =
    "Save any wanted replay first; restarting discards the current buffer.";
pub(super) const STARTUP_FAILED_TITLE: &str = "nvidia-mem-replay could not start";
pub(super) const STARTUP_FAILED_HELP: &str =
    "Close this window, resolve the error, then launch again.";

pub(super) fn log_file_note(path: &std::path::Path) -> String {
    format!("Also written to {}", path.display())
}

pub(super) fn tray_unavailable(error: &impl Display) -> String {
    format!("Tray unavailable; closing this window restores the path and quits: {error}")
}

pub(super) fn startup_setting_failed(error: &impl Display) -> String {
    format!("Could not update the Start with Windows entry: {error}")
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

pub(super) fn buffer_ceiling(bytes: u64) -> String {
    format!("{} MB", bytes / 1_000_000)
}
