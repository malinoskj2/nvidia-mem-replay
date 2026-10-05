#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub(crate) fn dispatch() -> anyhow::Result<bool> {
    windows::dispatch()
}
