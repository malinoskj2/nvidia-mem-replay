pub(super) const RECOVER_TEMP_PATH: &str = "recover NVIDIA's original temporary location; retry restoration or set Temporary files to a persistent drive in Alt+Z";
pub(super) const RESTORE_TEMP_PATH: &str = "restore NVIDIA temporary path; retry restoration or set Temporary files to a persistent drive in Alt+Z";
pub(super) const READ_REDIRECT_JOURNAL: &str = "read redirect recovery journal";
pub(super) const CLEAR_REDIRECT_JOURNAL: &str = "clear redirect recovery journal";

pub(super) const REDIRECTED_AGAIN: &str = "NVIDIA switched its temporary files back to the original location (its overlay restarted); they were redirected to RAM again.";

pub(super) fn redirect_again_failed(error: &str) -> String {
    format!(
        "NVIDIA switched its temporary files back to the original location (its overlay restarted) and they could not be redirected again: {error}"
    )
}

pub(super) fn replay_notice(error: &str) -> String {
    format!(
        "Instant Replay was not restarted automatically ({error}). Switch it off and on in Alt+Z so it records into the new location."
    )
}
