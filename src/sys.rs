pub(crate) mod helper;
#[cfg(windows)]
pub(crate) mod hotkey;
pub(crate) mod icon;
pub(crate) mod nvidia;
#[cfg(windows)]
pub(crate) mod startup;

/// The only module that calls into native code directly; see its documentation.
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod shadowplay;

#[cfg(windows)]
pub(crate) mod tray;
#[cfg(windows)]
pub(crate) mod window;
