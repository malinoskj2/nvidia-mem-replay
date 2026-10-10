//! The row under the tab, added only when it has something to say: the tray fallback hint, or
//! the shutdown progress with its retry and exit buttons.

use super::{
    TAB_HEIGHT,
    layout::{self, BLACK, GRAY, MARGIN, RED, WINDOW_WIDTH, button, label, set_text, show},
    text,
};
use winsafe::{self as w, co, gui, prelude::*};

/// Height of the row, in logical pixels.
pub(super) const HEIGHT: i32 = 36;

#[derive(Clone)]
pub(super) struct Footer {
    hint: gui::Label,
    shutdown_text: gui::Label,
    pub(super) retry: gui::Button,
    pub(super) exit: gui::Button,
}

impl Footer {
    pub(super) fn new(wnd: &gui::WindowMain) -> Self {
        let y = MARGIN + TAB_HEIGHT + 6;
        let footer = Self {
            hint: label(wnd, "", MARGIN + 2, y, WINDOW_WIDTH - 2 * MARGIN, 1),
            shutdown_text: label(wnd, "", MARGIN + 2, y, WINDOW_WIDTH - 2 * MARGIN - 220, 2),
            retry: button(
                wnd,
                text::RETRY_SHUTDOWN,
                WINDOW_WIDTH - MARGIN - 212,
                y,
                104,
            ),
            exit: button(wnd, text::EXIT_ANYWAY, WINDOW_WIDTH - MARGIN - 100, y, 100),
        };
        // The hint is muted and the shutdown text red; the window has no other labels.
        let colours = footer.clone();
        layout::draw(wnd, wnd.on(), Vec::new(), co::COLOR::BTNFACE, move |hwnd| {
            if *hwnd == *colours.shutdown_text.hwnd() {
                RED
            } else if *hwnd == *colours.hint.hwnd() {
                GRAY
            } else {
                BLACK
            }
        });
        footer
    }

    /// Everything in the row; hidden until the row is shown, and moved when the tab grows.
    pub(super) fn controls(&self) -> [&w::HWND; 4] {
        [
            self.hint.hwnd(),
            self.shutdown_text.hwnd(),
            self.retry.hwnd(),
            self.exit.hwnd(),
        ]
    }

    pub(super) fn show_hint(&self, message: &str) {
        set_text(&self.hint, message);
        show(&self.hint, true);
    }

    /// Replaces the hint with the shutdown progress.
    pub(super) fn begin_shutdown(&self) {
        show(&self.hint, false);
        set_text(&self.shutdown_text, text::SHUTDOWN_PENDING);
        show(&self.shutdown_text, true);
    }

    /// Reports the shutdown: a `failure` offers the retry and exit buttons, none says it is
    /// still under way.
    pub(super) fn show_shutdown(&self, failure: Option<&str>) {
        set_text(
            &self.shutdown_text,
            failure.unwrap_or(text::SHUTDOWN_PENDING),
        );
        show(&self.retry, failure.is_some());
        show(&self.exit, failure.is_some());
    }
}
