//! The desktop window: a plain Windows dialog made of the system's own common controls, so
//! fonts, tabs, group boxes and buttons are drawn by Windows exactly like any control panel.

use crate::{
    APP_NAME,
    config::{Config, MAX_MEMORY_LIMIT_MB, MIN_MEMORY_LIMIT_MB},
    service::{Shutdown, Status, Worker},
    sys::{
        startup,
        tray::{Tray, TrayAction},
        window,
    },
};
use anyhow::Result;
use std::{cell::RefCell, rc::Rc};
use winsafe::{self as w, co, gui, prelude::*};

mod text;

const STATUS_TIMER: usize = 1;
const STATUS_INTERVAL_MS: u32 = 250;
/// Resource id of the icon embedded by `build.rs`.
const ICON_RESOURCE: u16 = 1;

const WINDOW_WIDTH: i32 = 470;
const WINDOW_HEIGHT: i32 = 374;
const MARGIN: i32 = 8;
const TAB_HEIGHT: i32 = 324;
/// Height the Startup group gains while it shows a Run-key error line.
const STARTUP_ERROR_EXTRA: i32 = LINE + 4;
const STARTUP_FRAME: usize = 1;
const RECORDING_FRAME: usize = 2;
const PAGE_WIDTH: i32 = WINDOW_WIDTH - 2 * MARGIN - 8;
const GROUP_X: i32 = 8;
const GROUP_WIDTH: i32 = PAGE_WIDTH - 2 * GROUP_X;
const INNER_X: i32 = GROUP_X + 12;
const INNER_WIDTH: i32 = GROUP_WIDTH - 24;
const VALUE_X: i32 = INNER_X + 108;
const VALUE_WIDTH: i32 = INNER_WIDTH - 108;
const LINE: i32 = 18;
const BUTTON_HEIGHT: i32 = 24;

const BLACK: w::COLORREF = w::COLORREF::from_rgb(0x00, 0x00, 0x00);
const GRAY: w::COLORREF = w::COLORREF::from_rgb(0x6D, 0x6D, 0x6D);
const GREEN: w::COLORREF = w::COLORREF::from_rgb(0x10, 0x7C, 0x10);
const AMBER: w::COLORREF = w::COLORREF::from_rgb(0x9D, 0x5D, 0x00);
const RED: w::COLORREF = w::COLORREF::from_rgb(0xC4, 0x2B, 0x1C);

/// Shows the application window, or the startup error, until the user quits.
pub(crate) fn run(startup: Result<(Worker, Config)>, start_hidden: bool) -> Result<()> {
    match startup {
        Ok((worker, config)) => Main::create_and_run(worker, config, start_hidden),
        Err(error) => StartupError::create_and_run(&format!("{error:#}")),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Running,
    ShuttingDown,
    ExitReady,
}

struct Shared {
    worker: Worker,
    config: Config,
    lifecycle: Lifecycle,
    tray: Option<Tray>,
    quitting: bool,
    state_colour: w::COLORREF,
    path_colour: w::COLORREF,
    notice_colour: w::COLORREF,
    /// A Run-key error found before the window existed, shown once it does.
    initial_startup_error: Option<String>,
    startup_error_shown: bool,
}

#[derive(Clone)]
struct Main {
    wnd: gui::WindowMain,
    tab: gui::Tab,
    status: StatusPage,
    settings: SettingsPage,
    footer: gui::Label,
    shutdown_text: gui::Label,
    retry_shutdown: gui::Button,
    exit_anyway: gui::Button,
    shared: Rc<RefCell<Shared>>,
}

#[derive(Clone)]
struct StatusPage {
    page: gui::TabPage,
    frames: Vec<Frame>,
    state: gui::Label,
    written: gui::Label,
    buffer: gui::Label,
    notice: gui::Label,
    path_state: gui::Label,
    original: gui::Label,
    ram: gui::Label,
    ceiling: gui::Label,
}

#[derive(Clone)]
struct SettingsPage {
    page: gui::TabPage,
    /// Shared with the paint handler; the Startup frame grows while it shows an error.
    frames: Rc<RefCell<Vec<Frame>>>,
    ceiling: gui::Edit,
    _ceiling_spin: gui::UpDown,
    apply: gui::Button,
    settings_error: gui::Label,
    start_with_windows: gui::CheckBox,
    startup_error: gui::Label,
    recording_help: gui::Label,
    recording: gui::Button,
}

/// A group box: the themed frame is painted by the window itself (a child group-box control
/// never erases its interior, which leaves stale pixels under a clip-children parent) and the
/// title is an ordinary label sitting on the frame line.
#[derive(Clone)]
struct Frame {
    title: gui::Label,
    rect: w::RECT,
}

fn frame(parent: &(impl GuiParent + 'static), title: &str, y: i32, height: i32) -> Frame {
    let padded = format!(" {title} ");
    Frame {
        title: label(parent, &padded, GROUP_X + 8, y, 0, 1),
        rect: w::RECT {
            left: gui::dpi_x(GROUP_X),
            top: gui::dpi_y(y + LINE / 2),
            right: gui::dpi_x(GROUP_X + GROUP_WIDTH),
            bottom: gui::dpi_y(y + height),
        },
    }
}

/// Sizes the title labels to their text; only possible once the controls exist.
fn fit_titles(frames: &[Frame]) {
    for frame in frames {
        if let Ok(title) = frame.title.hwnd().GetWindowText() {
            let _ = frame.title.set_text_and_resize(&title);
        }
    }
}

fn paint_frames(hwnd: &w::HWND, frames: &[Frame]) {
    let Ok(paint) = hwnd.BeginPaint() else {
        return;
    };
    if let Some(theme) = hwnd.OpenThemeData("BUTTON") {
        for frame in frames {
            let _ =
                theme.DrawThemeBackground(&paint, co::VS::BUTTON_GROUPBOX_NORMAL, frame.rect, None);
        }
    }
}

fn label(
    parent: &(impl GuiParent + 'static),
    text: &str,
    x: i32,
    y: i32,
    width: i32,
    lines: i32,
) -> gui::Label {
    gui::Label::new(
        parent,
        gui::LabelOpts {
            text,
            position: gui::dpi(x, y),
            size: gui::dpi(width, LINE * lines),
            ..Default::default()
        },
    )
}

/// A single-line label that shortens a long path in the middle instead of clipping it.
fn path_label(parent: &(impl GuiParent + 'static), x: i32, y: i32, width: i32) -> gui::Label {
    gui::Label::new(
        parent,
        gui::LabelOpts {
            text: "",
            position: gui::dpi(x, y),
            size: gui::dpi(width, LINE),
            control_style: co::SS::LEFT | co::SS::PATHELLIPSIS,
            ..Default::default()
        },
    )
}

fn button(
    parent: &(impl GuiParent + 'static),
    text: &str,
    x: i32,
    y: i32,
    width: i32,
) -> gui::Button {
    gui::Button::new(
        parent,
        gui::ButtonOpts {
            text,
            position: gui::dpi(x, y),
            width: gui::dpi_x(width),
            height: gui::dpi_y(BUTTON_HEIGHT),
            ..Default::default()
        },
    )
}

impl StatusPage {
    fn new(parent: &(impl GuiParent + 'static)) -> Self {
        let page = gui::TabPage::new(parent, gui::TabPageOpts::default());

        let status_group = frame(&page, text::GROUP_STATUS, 6, 124);
        let state = label(&page, "", INNER_X, 26, INNER_WIDTH, 1);
        let _written_caption = label(
            &page,
            text::LIFETIME_WRITES,
            INNER_X,
            48,
            VALUE_X - INNER_X,
            1,
        );
        let written = label(&page, "", VALUE_X, 48, VALUE_WIDTH, 1);
        let _buffer_caption = label(
            &page,
            text::ALLOCATED_BUFFER,
            INNER_X,
            68,
            VALUE_X - INNER_X,
            1,
        );
        let buffer = label(&page, "", VALUE_X, 68, VALUE_WIDTH, 1);
        let notice = label(&page, "", INNER_X, 90, INNER_WIDTH, 2);

        let locations_group = frame(&page, text::GROUP_LOCATIONS, 138, 112);
        let path_state = label(&page, "", INNER_X, 158, INNER_WIDTH, 1);
        let _original_caption = label(
            &page,
            text::ORIGINAL_LOCATION,
            INNER_X,
            180,
            VALUE_X - INNER_X,
            1,
        );
        let original = path_label(&page, VALUE_X, 180, VALUE_WIDTH);
        let _ram_caption = label(
            &page,
            text::RAM_LOCATION,
            INNER_X,
            200,
            VALUE_X - INNER_X,
            1,
        );
        let ram = path_label(&page, VALUE_X, 200, VALUE_WIDTH);
        let _ceiling_caption = label(
            &page,
            text::BUFFER_CEILING,
            INNER_X,
            220,
            VALUE_X - INNER_X,
            1,
        );
        let ceiling = label(&page, "", VALUE_X, 220, VALUE_WIDTH, 1);

        let frames = vec![status_group, locations_group];
        let (paint_page, paint_frames_list) = (page.clone(), frames.clone());
        page.on().wm_paint(move || {
            paint_frames(paint_page.hwnd(), &paint_frames_list);
            Ok(())
        });

        Self {
            page,
            frames,
            state,
            written,
            buffer,
            notice,
            path_state,
            original,
            ram,
            ceiling,
        }
    }
}

/// The controls of the "RAM storage" group.
struct StorageControls {
    group: Frame,
    ceiling: gui::Edit,
    ceiling_spin: gui::UpDown,
    apply: gui::Button,
    settings_error: gui::Label,
}

impl SettingsPage {
    fn new(parent: &(impl GuiParent + 'static), config: &Config, start_with_windows: bool) -> Self {
        let page = gui::TabPage::new(parent, gui::TabPageOpts::default());
        let storage = Self::storage_controls(&page, config);

        // Compact; `show_startup_error` adds a line for the error label when one occurs.
        let startup_group = frame(&page, text::GROUP_STARTUP, 120, 78);
        let start_with_windows = gui::CheckBox::new(
            &page,
            gui::CheckBoxOpts {
                text: text::START_WITH_WINDOWS,
                position: gui::dpi(INNER_X, 140),
                check_state: if start_with_windows {
                    co::BST::CHECKED
                } else {
                    co::BST::UNCHECKED
                },
                ..Default::default()
            },
        );
        let _startup_help = label(
            &page,
            text::START_WITH_WINDOWS_HELP,
            INNER_X,
            160,
            INNER_WIDTH,
            2,
        );
        let startup_error = label(&page, "", INNER_X, 160 + 2 * LINE, INNER_WIDTH, 1);

        let recording_group = frame(&page, text::GROUP_RECORDING, 206, 90);
        let recording_help = label(&page, "", INNER_X, 226, INNER_WIDTH, 2);
        let recording = button(&page, text::STOP_AND_RESTORE, INNER_X, 264, 120);

        let frames = Rc::new(RefCell::new(vec![
            storage.group,
            startup_group,
            recording_group,
        ]));
        let (paint_page, paint_frames_list) = (page.clone(), Rc::clone(&frames));
        page.on().wm_paint(move || {
            paint_frames(paint_page.hwnd(), &paint_frames_list.borrow());
            Ok(())
        });

        Self {
            page,
            frames,
            ceiling: storage.ceiling,
            _ceiling_spin: storage.ceiling_spin,
            apply: storage.apply,
            settings_error: storage.settings_error,
            start_with_windows,
            startup_error,
            recording_help,
            recording,
        }
    }

    fn storage_controls(page: &gui::TabPage, config: &Config) -> StorageControls {
        let group = frame(page, text::GROUP_STORAGE, 6, 106);
        let _ceiling_caption = label(page, text::MEMORY_CEILING, INNER_X, 30, 76, 1);
        let ceiling_text = config.memory_limit_mb.to_string();
        let ceiling = gui::Edit::new(
            page,
            gui::EditOpts {
                text: &ceiling_text,
                position: gui::dpi(INNER_X + 80, 26),
                width: gui::dpi_x(76),
                control_style: co::ES::NUMBER | co::ES::AUTOHSCROLL | co::ES::NOHIDESEL,
                ..Default::default()
            },
        );
        let ceiling_spin = gui::UpDown::new(
            page,
            gui::UpDownOpts {
                range: (
                    i32::try_from(MIN_MEMORY_LIMIT_MB).unwrap_or(i32::MAX),
                    i32::try_from(MAX_MEMORY_LIMIT_MB).unwrap_or(i32::MAX),
                ),
                value: i32::try_from(config.memory_limit_mb).unwrap_or(i32::MAX),
                // No thousands separators: the field is parsed back as a plain number.
                control_style: co::UDS::AUTOBUDDY
                    | co::UDS::SETBUDDYINT
                    | co::UDS::ALIGNRIGHT
                    | co::UDS::ARROWKEYS
                    | co::UDS::HOTTRACK
                    | co::UDS::NOTHOUSANDS,
                ..Default::default()
            },
        );
        let _restart_notice = label(page, text::RESTART_NOTICE, INNER_X, 54, INNER_WIDTH, 1);
        let apply = button(page, text::APPLY_AND_RESTART, INNER_X, 76, 120);
        let settings_error = label(page, "", INNER_X + 128, 80, INNER_WIDTH - 128, 1);

        StorageControls {
            group,
            ceiling,
            ceiling_spin,
            apply,
            settings_error,
        }
    }

    /// Makes the Startup group `delta` pixels taller (negative to shrink) and moves the
    /// Recording group with it.
    fn grow_startup(&self, delta: i32) {
        {
            let mut frames = self.frames.borrow_mut();
            frames[STARTUP_FRAME].rect.bottom += delta;
            frames[RECORDING_FRAME].rect.top += delta;
            frames[RECORDING_FRAME].rect.bottom += delta;
            shift_by(frames[RECORDING_FRAME].title.hwnd(), delta);
        }
        shift_by(self.recording_help.hwnd(), delta);
        shift_by(self.recording.hwnd(), delta);
        let _ = self.page.hwnd().InvalidateRect(None, true);
    }

    /// The configuration as currently entered, or the problem with it.
    fn configuration(&self) -> std::result::Result<Config, String> {
        let memory_limit_mb = self
            .ceiling
            .text()
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .ok_or_else(|| text::CEILING_REQUIRED.to_owned())?;

        let config = Config { memory_limit_mb };
        config.validate().map_err(|error| error.to_string())?;
        Ok(config)
    }
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
        let tab = gui::Tab::new(
            &wnd,
            gui::TabOpts {
                position: gui::dpi(MARGIN, MARGIN),
                size: gui::dpi(WINDOW_WIDTH - 2 * MARGIN, TAB_HEIGHT),
                pages: &[
                    (text::TAB_STATUS, status.page.clone()),
                    (text::TAB_SETTINGS, settings.page.clone()),
                ],
                ..Default::default()
            },
        );
        let footer_y = MARGIN + TAB_HEIGHT + 6;
        let footer = label(&wnd, "", MARGIN + 2, footer_y, WINDOW_WIDTH - 2 * MARGIN, 1);
        let shutdown_text = label(
            &wnd,
            "",
            MARGIN + 2,
            footer_y,
            WINDOW_WIDTH - 2 * MARGIN - 220,
            2,
        );
        let retry_shutdown = button(
            &wnd,
            text::RETRY_SHUTDOWN,
            WINDOW_WIDTH - MARGIN - 212,
            footer_y,
            104,
        );
        let exit_anyway = button(
            &wnd,
            text::EXIT_ANYWAY,
            WINDOW_WIDTH - MARGIN - 100,
            footer_y,
            100,
        );

        let main = Self {
            wnd,
            tab,
            status,
            settings,
            footer,
            shutdown_text,
            retry_shutdown,
            exit_anyway,
            shared: Rc::new(RefCell::new(Shared {
                worker,
                config,
                lifecycle: Lifecycle::Running,
                tray: None,
                quitting: false,
                state_colour: GRAY,
                path_colour: GRAY,
                notice_colour: AMBER,
                initial_startup_error: startup_error,
                startup_error_shown: false,
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

        let me = self.clone();
        self.wnd
            .on()
            .wm_ctl_color_static(move |p| Ok(me.colour_label(&p, co::COLOR::BTNFACE)));
        let me = self.clone();
        self.status
            .page
            .on()
            .wm_ctl_color_static(move |p| Ok(me.colour_label(&p, co::COLOR::WINDOW)));
        let me = self.clone();
        self.settings
            .page
            .on()
            .wm_ctl_color_static(move |p| Ok(me.colour_label(&p, co::COLOR::WINDOW)));

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
            me.toggle_startup();
            Ok(())
        });

        let me = self.clone();
        self.retry_shutdown.on().bn_clicked(move || {
            me.shared.borrow().worker.shutdown();
            Ok(())
        });

        let me = self.clone();
        self.exit_anyway.on().bn_clicked(move || {
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
        fit_titles(&self.status.frames);
        fit_titles(&self.settings.frames.borrow());
        let _ = self
            .wnd
            .hwnd()
            .SetTimer(STATUS_TIMER, STATUS_INTERVAL_MS, None);
        self.retry_shutdown.hwnd().ShowWindow(co::SW::HIDE);
        self.exit_anyway.hwnd().ShowWindow(co::SW::HIDE);
        self.shutdown_text.hwnd().ShowWindow(co::SW::HIDE);
        self.settings.startup_error.hwnd().ShowWindow(co::SW::HIDE);
        let initial_error = self.shared.borrow_mut().initial_startup_error.take();
        if let Some(error) = initial_error {
            self.show_startup_error(Some(&error));
        }

        let me = self.clone();
        match Tray::new(move |action| me.on_tray(action)) {
            Ok(tray) => {
                self.shared.borrow_mut().tray = Some(tray);
                let _ = self.footer.hwnd().SetWindowText(text::CLOSE_WITH_TRAY);
            }
            Err(error) => {
                let _ = self
                    .footer
                    .hwnd()
                    .SetWindowText(&text::tray_unavailable(&error));
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
        self.footer.hwnd().ShowWindow(co::SW::HIDE);
        let _ = self
            .shutdown_text
            .hwnd()
            .SetWindowText(text::SHUTDOWN_PENDING);
        self.shutdown_text.hwnd().ShowWindow(co::SW::SHOW);
    }

    fn refresh(&self) {
        let status = self.shared.borrow().worker.status();
        self.track_shutdown(&status);

        let (state_colour, path_colour) = (
            if status.active { GREEN } else { GRAY },
            if status.mounted { GREEN } else { GRAY },
        );
        let notices: Vec<&str> = [
            status.warning.as_deref().filter(|_| status.mounted),
            status.notice.as_deref(),
            status.error.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let notice_colour = if status.error.is_some() { RED } else { AMBER };
        {
            let mut shared = self.shared.borrow_mut();
            let recolour = shared.state_colour != state_colour
                || shared.path_colour != path_colour
                || shared.notice_colour != notice_colour;
            shared.state_colour = state_colour;
            shared.path_colour = path_colour;
            shared.notice_colour = notice_colour;
            if recolour {
                for control in [
                    &self.status.state,
                    &self.status.path_state,
                    &self.status.notice,
                ] {
                    let _ = control.hwnd().InvalidateRect(None, true);
                }
            }
        }

        let limit = status
            .memory_limit_bytes
            .unwrap_or_else(|| self.shared.borrow().config.limit_bytes());
        set_text(&self.status.state, &text::status(status.message.as_str()));
        set_text(
            &self.status.written,
            &text::lifetime_written(status.lifetime_bytes),
        );
        set_text(
            &self.status.buffer,
            &text::buffer_allocated(status.sample.as_ref().map(|sample| sample.buffer_bytes)),
        );
        set_text(&self.status.notice, &notices.join("\r\n"));
        set_text(
            &self.status.path_state,
            &text::status(if status.mounted {
                text::PATH_SWAPPED
            } else {
                text::PATH_ORIGINAL
            }),
        );
        set_text(
            &self.status.original,
            status.original_path.as_deref().unwrap_or("—"),
        );
        set_text(&self.status.ram, status.target.as_deref().unwrap_or("—"));
        set_text(&self.status.ceiling, &text::buffer_ceiling(limit));

        let (recording_label, recording_help) = if status.mounted {
            (text::STOP_AND_RESTORE, text::STOP_HELP)
        } else {
            (text::RETRY_START, text::START_HELP)
        };
        set_text(&self.settings.recording_help, recording_help);
        if self.settings.recording.hwnd().GetWindowText().as_deref() != Ok(recording_label) {
            let _ = self
                .settings
                .recording
                .hwnd()
                .SetWindowText(recording_label);
        }
    }

    fn track_shutdown(&self, status: &Status) {
        let lifecycle = self.shared.borrow().lifecycle;
        if lifecycle != Lifecycle::ShuttingDown {
            return;
        }

        match status.shutdown {
            Shutdown::Complete => {
                self.shared.borrow_mut().lifecycle = Lifecycle::ExitReady;
                let _ = self.wnd.hwnd().DestroyWindow();
            }
            Shutdown::Failed => {
                let message = status
                    .error
                    .as_deref()
                    .unwrap_or(text::SHUTDOWN_FAILED_HELP);
                set_text(&self.shutdown_text, message);
                self.retry_shutdown.hwnd().ShowWindow(co::SW::SHOW);
                self.exit_anyway.hwnd().ShowWindow(co::SW::SHOW);
            }
            Shutdown::Idle | Shutdown::Pending => {
                set_text(&self.shutdown_text, text::SHUTDOWN_PENDING);
                self.retry_shutdown.hwnd().ShowWindow(co::SW::HIDE);
                self.exit_anyway.hwnd().ShowWindow(co::SW::HIDE);
            }
        }
    }

    fn apply_settings(&self) {
        match self.settings.configuration() {
            Ok(config) => {
                set_text(&self.settings.settings_error, "");
                let mut shared = self.shared.borrow_mut();
                shared.config = config.clone();
                shared.worker.start(config);
            }
            Err(error) => set_text(&self.settings.settings_error, &error),
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

    fn toggle_startup(&self) {
        let wanted = self.settings.start_with_windows.is_checked();
        match startup::set(wanted) {
            Ok(()) => self.show_startup_error(None),
            Err(error) => {
                self.settings.start_with_windows.set_check(!wanted);
                self.show_startup_error(Some(&text::startup_setting_failed(&error)));
            }
        }
    }

    /// The Startup group stays compact until a Run-key error needs its extra line; the
    /// Recording group, the tab and the window grow with it and shrink back once it clears.
    fn show_startup_error(&self, message: Option<&str>) {
        set_text(&self.settings.startup_error, message.unwrap_or(""));
        let shown = message.is_some();
        self.settings.startup_error.hwnd().ShowWindow(if shown {
            co::SW::SHOW
        } else {
            co::SW::HIDE
        });

        let changed = {
            let mut shared = self.shared.borrow_mut();
            let changed = shared.startup_error_shown != shown;
            shared.startup_error_shown = shown;
            changed
        };
        if !changed {
            return;
        }

        let delta = gui::dpi_y(if shown {
            STARTUP_ERROR_EXTRA
        } else {
            -STARTUP_ERROR_EXTRA
        });
        self.settings.grow_startup(delta);
        for window in [
            self.wnd.hwnd(),
            self.tab.hwnd(),
            self.status.page.hwnd(),
            self.settings.page.hwnd(),
        ] {
            resize_by(window, delta);
        }
        for window in [
            self.footer.hwnd(),
            self.shutdown_text.hwnd(),
            self.retry_shutdown.hwnd(),
            self.exit_anyway.hwnd(),
        ] {
            shift_by(window, delta);
        }
        // Moved controls and the strip the shorter window uncovers keep stale pixels otherwise.
        if let Ok(client) = self.wnd.hwnd().GetClientRect() {
            let _ = self.wnd.hwnd().RedrawWindow(
                client,
                &w::HRGN::NULL,
                co::RDW::INVALIDATE | co::RDW::ERASE | co::RDW::ALLCHILDREN,
            );
        }
    }

    /// Static controls ask their parent for colours; a few labels are tinted by meaning.
    fn colour_label(&self, p: &w::msg::WmCtlColorStatic, background: co::COLOR) -> w::HBRUSH {
        let shared = self.shared.borrow();
        let colour = if p.hwnd == *self.status.state.hwnd() {
            shared.state_colour
        } else if p.hwnd == *self.status.path_state.hwnd() {
            shared.path_colour
        } else if p.hwnd == *self.status.notice.hwnd() {
            shared.notice_colour
        } else if p.hwnd == *self.settings.settings_error.hwnd()
            || p.hwnd == *self.settings.startup_error.hwnd()
            || p.hwnd == *self.shutdown_text.hwnd()
        {
            RED
        } else if p.hwnd == *self.footer.hwnd() {
            GRAY
        } else {
            BLACK
        };
        let _ = p.hdc.SetTextColor(colour);
        let _ = p.hdc.SetBkMode(co::BKMODE::TRANSPARENT);
        w::HBRUSH::GetSysColorBrush(background).unwrap_or(w::HBRUSH::NULL)
    }
}

/// Moves a child window down by `delta` pixels (up when negative).
fn shift_by(window: &w::HWND, delta: i32) {
    let Ok(parent) = window.GetParent() else {
        return;
    };
    let Ok(rect) = window
        .GetWindowRect()
        .and_then(|rect| parent.ScreenToClientRc(rect))
    else {
        return;
    };
    let _ = window.SetWindowPos(
        w::HwndPlace::None,
        w::POINT::with(rect.left, rect.top + delta),
        w::SIZE::default(),
        co::SWP::NOSIZE | co::SWP::NOZORDER | co::SWP::NOACTIVATE | co::SWP::NOCOPYBITS,
    );
}

/// Makes a window `delta` pixels taller (shorter when negative), keeping its position.
fn resize_by(window: &w::HWND, delta: i32) {
    let Ok(rect) = window.GetWindowRect() else {
        return;
    };
    let _ = window.SetWindowPos(
        w::HwndPlace::None,
        w::POINT::default(),
        w::SIZE::with(rect.right - rect.left, rect.bottom - rect.top + delta),
        co::SWP::NOMOVE | co::SWP::NOZORDER | co::SWP::NOACTIVATE | co::SWP::NOCOPYBITS,
    );
}

fn set_text(label: &gui::Label, value: &str) {
    if label.hwnd().GetWindowText().as_deref() != Ok(value) {
        let _ = label.hwnd().SetWindowText(value);
    }
}

#[derive(Clone)]
struct StartupError {
    wnd: gui::WindowMain,
    message: gui::Label,
}

impl StartupError {
    fn create_and_run(message: &str) -> Result<()> {
        let wnd = gui::WindowMain::new(gui::WindowMainOpts {
            title: APP_NAME,
            size: gui::dpi(WINDOW_WIDTH, 170),
            class_icon: gui::Icon::Id(ICON_RESOURCE),
            ..Default::default()
        });
        let group = frame(&wnd, text::STARTUP_FAILED_TITLE, 6, 140);
        let message_label = label(&wnd, message, INNER_X, 28, INNER_WIDTH, 4);
        let _help = label(
            &wnd,
            text::STARTUP_FAILED_HELP,
            INNER_X,
            28 + 4 * LINE + 6,
            INNER_WIDTH,
            1,
        );

        let window = Self {
            wnd,
            message: message_label,
        };
        let me = window.clone();
        let frames = vec![group];
        let title_frames = frames.clone();
        window.wnd.on().wm_create(move |_| {
            window::match_dialog_title_bar(me.wnd.hwnd());
            fit_titles(&title_frames);
            Ok(0)
        });
        let paint_wnd = window.wnd.clone();
        window.wnd.on().wm_paint(move || {
            paint_frames(paint_wnd.hwnd(), &frames);
            Ok(())
        });
        let me = window.clone();
        window.wnd.on().wm_ctl_color_static(move |p| {
            let _ = p.hdc.SetTextColor(if p.hwnd == *me.message.hwnd() {
                RED
            } else {
                BLACK
            });
            let _ = p.hdc.SetBkMode(co::BKMODE::TRANSPARENT);
            Ok(w::HBRUSH::GetSysColorBrush(co::COLOR::BTNFACE).unwrap_or(w::HBRUSH::NULL))
        });

        window
            .wnd
            .run_main(None)
            .map(|_| ())
            .map_err(|error| anyhow::anyhow!("show the startup error: {error}"))
    }
}
