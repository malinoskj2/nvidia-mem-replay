//! The desktop window: a plain Windows dialog made of the system's own common controls, so
//! fonts, tabs, group boxes and buttons are drawn by Windows exactly like any control panel.

use crate::{
    APP_NAME,
    config::Config,
    log,
    service::{Shutdown, Status, Worker},
    sys::{
        startup,
        tray::{Tray, TrayAction},
        window,
    },
};
use anyhow::Result;
use layout::{
    BLACK, GRAY, MARGIN, RED, WINDOW_WIDTH, button, label, resize_by, set_text, shift_by, show,
};
use logs::LogsPage;
use settings::SettingsPage;
use status::StatusPage;
use std::{cell::RefCell, rc::Rc};
use winsafe::{self as w, co, gui, prelude::*};

mod layout;
mod logs;
mod settings;
mod startup_error;
mod status;
mod text;

const STATUS_TIMER: usize = 1;
const STATUS_INTERVAL_MS: u32 = 250;
/// Resource id of the icon embedded by `build.rs`.
const ICON_RESOURCE: u16 = 1;

/// The tab and its margins; a footer row is added only while it has something to say.
const WINDOW_HEIGHT: i32 = MARGIN + TAB_HEIGHT + MARGIN;
const TAB_HEIGHT: i32 = 324;
/// Height of the footer row that shows the tray fallback or the shutdown controls.
const FOOTER_EXTRA: i32 = 36;

/// Shows the application window, or the startup error, until the user quits.
pub(crate) fn run(startup: Result<(Worker, Config)>, start_hidden: bool) -> Result<()> {
    match startup {
        Ok((worker, config)) => Main::create_and_run(worker, config, start_hidden),
        Err(error) => startup_error::show(&format!("{error:#}")),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Running,
    ShuttingDown,
    ExitReady,
}

/// What the handlers share. Every borrow is short-lived and none is taken while painting:
/// the colours the paint-time handlers read live beside their labels.
struct Shared {
    worker: Worker,
    config: Config,
    lifecycle: Lifecycle,
    tray: Option<Tray>,
    quitting: bool,
    /// A Run-key error found before the window existed, shown once it does.
    initial_startup_error: Option<String>,
    heights: Heights,
}

/// The height added to the fixed layout: the tab and its pages get the larger of the pages'
/// expanded groups, the window that plus the footer row once it is shown.
#[derive(Default)]
struct Heights {
    footer: i32,
    pages: i32,
    window: i32,
}

impl Heights {
    /// Records what the pages now need and returns how much the pages and the window must
    /// change, in logical pixels.
    fn fit(&mut self, pages: i32) -> (i32, i32) {
        let window = pages + self.footer;
        let deltas = (pages - self.pages, window - self.window);
        self.pages = pages;
        self.window = window;
        deltas
    }
}

/// The row under the tab, added only when it has something to say: the tray fallback hint,
/// or the shutdown progress with its retry and exit buttons.
#[derive(Clone)]
struct Footer {
    hint: gui::Label,
    shutdown_text: gui::Label,
    retry: gui::Button,
    exit: gui::Button,
}

impl Footer {
    fn new(wnd: &gui::WindowMain) -> Self {
        let y = MARGIN + TAB_HEIGHT + 6;
        Self {
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
        }
    }

    fn controls(&self) -> [&w::HWND; 4] {
        [
            self.hint.hwnd(),
            self.shutdown_text.hwnd(),
            self.retry.hwnd(),
            self.exit.hwnd(),
        ]
    }

    /// The hint is muted and the shutdown text red; the window has no other labels.
    fn text_colour(&self, hwnd: &w::HWND) -> w::COLORREF {
        if *hwnd == *self.shutdown_text.hwnd() {
            RED
        } else if *hwnd == *self.hint.hwnd() {
            GRAY
        } else {
            BLACK
        }
    }

    fn show_hint(&self, message: &str) {
        set_text(&self.hint, message);
        show(&self.hint, true);
    }

    /// Replaces the hint with the shutdown progress.
    fn begin_shutdown(&self) {
        show(&self.hint, false);
        set_text(&self.shutdown_text, text::SHUTDOWN_PENDING);
        show(&self.shutdown_text, true);
    }

    /// Reports the shutdown: a `failure` offers the retry and exit buttons, none says it is
    /// still under way.
    fn show_shutdown(&self, failure: Option<&str>) {
        set_text(
            &self.shutdown_text,
            failure.unwrap_or(text::SHUTDOWN_PENDING),
        );
        show(&self.retry, failure.is_some());
        show(&self.exit, failure.is_some());
    }
}

#[derive(Clone)]
struct Main {
    wnd: gui::WindowMain,
    tab: gui::Tab,
    status: StatusPage,
    settings: SettingsPage,
    logs: LogsPage,
    footer: Footer,
    shared: Rc<RefCell<Shared>>,
}

impl Main {
    fn create_and_run(worker: Worker, config: Config, start_hidden: bool) -> Result<()> {
        // A window created visible would flash before `--tray` hides it.
        let visibility = if start_hidden {
            co::WS::NoValue
        } else {
            co::WS::VISIBLE
        };
        let wnd = gui::WindowMain::new(gui::WindowMainOpts {
            title: APP_NAME,
            size: gui::dpi(WINDOW_WIDTH, WINDOW_HEIGHT),
            class_icon: gui::Icon::Id(ICON_RESOURCE),
            style: co::WS::CAPTION
                | co::WS::SYSMENU
                | co::WS::MINIMIZEBOX
                | co::WS::CLIPCHILDREN
                | co::WS::BORDER
                | visibility,
            ..Default::default()
        });

        let (start_with_windows, startup_error) = match startup::enabled() {
            Ok(enabled) => (enabled, None),
            Err(error) => (false, Some(text::startup_setting_failed(&error))),
        };
        let status = StatusPage::new(&wnd);
        let settings = SettingsPage::new(&wnd, &config, start_with_windows);
        let logs = LogsPage::new(&wnd);
        let tab = gui::Tab::new(
            &wnd,
            gui::TabOpts {
                position: gui::dpi(MARGIN, MARGIN),
                size: gui::dpi(WINDOW_WIDTH - 2 * MARGIN, TAB_HEIGHT),
                pages: &[
                    (text::TAB_STATUS, status.page().clone()),
                    (text::TAB_SETTINGS, settings.page().clone()),
                    (text::TAB_LOGS, logs.page().clone()),
                ],
                ..Default::default()
            },
        );
        let footer = Footer::new(&wnd);

        let main = Self {
            wnd,
            tab,
            status,
            settings,
            logs,
            footer,
            shared: Rc::new(RefCell::new(Shared {
                worker,
                config,
                lifecycle: Lifecycle::Running,
                tray: None,
                quitting: false,
                initial_startup_error: startup_error,
                heights: Heights::default(),
            })),
        };
        main.events();

        let show = if start_hidden {
            co::SW::HIDE
        } else {
            co::SW::SHOW
        };
        main.wnd
            .run_main(Some(show))
            .map(|_| ())
            .map_err(|error| anyhow::anyhow!("run the application window: {error}"))
    }

    fn events(&self) {
        let me = self.clone();
        self.wnd.on().wm_create(move |_| {
            me.on_create();
            Ok(0)
        });

        let me = self.clone();
        self.wnd.on().wm_timer(STATUS_TIMER, move || {
            me.refresh();
            Ok(())
        });

        let me = self.clone();
        self.wnd.on().wm_close(move || {
            me.on_close();
            Ok(())
        });

        let me = self.clone();
        self.wnd.on().wm_destroy(move || {
            // Breaks the reference cycle through the tray's callback so the worker is dropped.
            me.shared.borrow_mut().tray = None;
            Ok(())
        });

        let footer = self.footer.clone();
        layout::draw(
            &self.wnd,
            self.wnd.on(),
            Vec::new(),
            co::COLOR::BTNFACE,
            move |hwnd| footer.text_colour(hwnd),
        );

        let me = self.clone();
        self.settings.apply.on().bn_clicked(move || {
            me.apply_settings();
            Ok(())
        });

        let me = self.clone();
        self.settings.recording.on().bn_clicked(move || {
            me.toggle_recording();
            Ok(())
        });

        let me = self.clone();
        self.settings.start_with_windows.on().bn_clicked(move || {
            if me.settings.toggle_startup() {
                me.fit_window();
            }
            Ok(())
        });

        let me = self.clone();
        self.footer.retry.on().bn_clicked(move || {
            me.shared.borrow().worker.shutdown();
            Ok(())
        });

        let me = self.clone();
        self.footer.exit.on().bn_clicked(move || {
            {
                let mut shared = me.shared.borrow_mut();
                shared.worker.exit();
                shared.lifecycle = Lifecycle::ExitReady;
            }
            let _ = me.wnd.hwnd().DestroyWindow();
            Ok(())
        });
    }

    fn on_create(&self) {
        window::match_dialog_title_bar(self.wnd.hwnd());
        self.status.on_create();
        self.settings.on_create();
        self.logs.on_create();
        let _ = self
            .wnd
            .hwnd()
            .SetTimer(STATUS_TIMER, STATUS_INTERVAL_MS, None);
        for control in self.footer.controls() {
            control.ShowWindow(co::SW::HIDE);
        }
        let initial_error = self.shared.borrow_mut().initial_startup_error.take();
        if let Some(error) = initial_error
            && self.settings.show_startup_error(Some(&error))
        {
            self.fit_window();
        }

        let me = self.clone();
        match Tray::new(move |action| me.on_tray(action)) {
            Ok(tray) => self.shared.borrow_mut().tray = Some(tray),
            Err(error) => {
                // Without a tray, closing quits; the footer row appears to say so.
                let message = text::tray_unavailable(&error);
                log::warning(&message);
                self.footer.show_hint(&message);
                self.show_footer_row();
            }
        }
        self.refresh();
    }

    fn on_tray(&self, action: TrayAction) {
        match action {
            TrayAction::Show => self.show(),
            TrayAction::Stop => {
                self.shared.borrow().worker.stop();
                self.show();
            }
            TrayAction::Quit => {
                log::info("Quit chosen from the tray menu");
                self.shared.borrow_mut().quitting = true;
                self.on_close();
            }
        }
    }

    fn show(&self) {
        self.wnd.hwnd().ShowWindow(co::SW::RESTORE);
        self.wnd.hwnd().SetForegroundWindow();
    }

    /// Closing hides to the tray; quitting (or no tray) starts the orderly shutdown and the
    /// window is destroyed once the worker reports it complete.
    fn on_close(&self) {
        let mut shared = self.shared.borrow_mut();
        if shared.lifecycle != Lifecycle::Running {
            return;
        }
        if shared.tray.is_some() && !shared.quitting {
            drop(shared);
            log::info("Window closed; recording continues from the tray");
            self.wnd.hwnd().ShowWindow(co::SW::HIDE);
            return;
        }

        shared.lifecycle = Lifecycle::ShuttingDown;
        shared.worker.shutdown();
        if let Some(tray) = &shared.tray {
            tray.disable_recording_controls();
        }
        drop(shared);

        self.show();
        self.tab.hwnd().EnableWindow(false);
        self.footer.begin_shutdown();
        self.show_footer_row();
    }

    /// Adds the footer row under the tab; it is never taken away again.
    fn show_footer_row(&self) {
        self.shared.borrow_mut().heights.footer = FOOTER_EXTRA;
        self.fit_window();
    }

    fn refresh(&self) {
        let status = self.shared.borrow().worker.status();
        self.track_shutdown(&status);
        let limit = status
            .memory_limit_bytes
            .unwrap_or_else(|| self.shared.borrow().config.limit_bytes());
        if self.status.update(&status, limit) {
            self.fit_window();
        }
        self.settings.update(&status);
        self.logs.update();
    }

    fn track_shutdown(&self, status: &Status) {
        if self.shared.borrow().lifecycle != Lifecycle::ShuttingDown {
            return;
        }
        match status.shutdown {
            Shutdown::Complete => {
                self.shared.borrow_mut().lifecycle = Lifecycle::ExitReady;
                let _ = self.wnd.hwnd().DestroyWindow();
            }
            Shutdown::Failed => self.footer.show_shutdown(Some(
                status
                    .error
                    .as_deref()
                    .unwrap_or(text::SHUTDOWN_FAILED_HELP),
            )),
            Shutdown::Idle | Shutdown::Pending => self.footer.show_shutdown(None),
        }
    }

    fn apply_settings(&self) {
        match self.settings.configuration() {
            Ok(config) => {
                self.settings.show_settings_error("");
                let mut shared = self.shared.borrow_mut();
                shared.config = config.clone();
                shared.worker.start(config);
            }
            Err(error) => self.settings.show_settings_error(&error),
        }
    }

    fn toggle_recording(&self) {
        let status = self.shared.borrow().worker.status();
        let shared = self.shared.borrow();
        if status.mounted {
            shared.worker.stop();
        } else {
            shared.worker.start(shared.config.clone());
        }
    }

    /// Gives the tab and the pages the larger of the pages' extra heights, moving the footer
    /// controls along; the window gets that plus the footer row when it is shown. Repaints.
    fn fit_window(&self) {
        let pages = self.status.extra().max(self.settings.extra());
        let (page_delta, window_delta) = self.shared.borrow_mut().heights.fit(pages);
        let (page_delta, window_delta) = (gui::dpi_y(page_delta), gui::dpi_y(window_delta));
        if page_delta != 0 {
            for window in [
                self.tab.hwnd(),
                self.status.page().hwnd(),
                self.settings.page().hwnd(),
                self.logs.page().hwnd(),
                self.logs.view().hwnd(),
            ] {
                resize_by(window, page_delta);
            }
            for window in self.footer.controls() {
                shift_by(window, page_delta);
            }
        }
        if window_delta != 0 {
            resize_by(self.wnd.hwnd(), window_delta);
        }
        // Moved controls and the strip a shorter window uncovers keep stale pixels otherwise.
        if let Ok(client) = self.wnd.hwnd().GetClientRect() {
            let _ = self.wnd.hwnd().RedrawWindow(
                client,
                &w::HRGN::NULL,
                co::RDW::INVALIDATE | co::RDW::ERASE | co::RDW::ALLCHILDREN,
            );
        }
    }
}
