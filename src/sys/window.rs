//! Native window chrome: a flat dialog-coloured title bar instead of Windows 11's
//! wallpaper-tinted backdrop, matching classic control-panel applets.

use crate::APP_NAME;
use winsafe::{self as w, co};

/// The caption colour of an ordinary light-theme dialog.
const CAPTION: w::COLORREF = w::COLORREF::from_rgb(0xF3, 0xF3, 0xF3);
const CAPTION_TEXT: w::COLORREF = w::COLORREF::from_rgb(0x00, 0x00, 0x00);

/// Applies the dialog title bar to this thread's application window.
pub(crate) fn match_dialog_title_bar() {
    with_application_window(|window| {
        // Older Windows versions lack these attributes; the default chrome is then kept.
        let _ = window.DwmSetWindowAttribute(w::DwmAttr::SystemBackdropType(co::DWMSBT::NONE));
        let _ = window.DwmSetWindowAttribute(w::DwmAttr::CaptionColor(CAPTION));
        let _ = window.DwmSetWindowAttribute(w::DwmAttr::TextColor(CAPTION_TEXT));
    });
}

fn with_application_window(apply: impl Fn(&w::HWND)) {
    let _ = w::EnumThreadWindows(w::GetCurrentThreadId(), |window| {
        if window.GetWindowText().is_ok_and(|title| title == APP_NAME) {
            apply(&window);
            return false;
        }

        true
    });
}
