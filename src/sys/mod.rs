pub(crate) mod helper;
pub(crate) mod nvidia;

/// The only module that calls into native code directly; see its documentation.
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod shadowplay;

#[cfg(windows)]
pub(crate) mod tray;
