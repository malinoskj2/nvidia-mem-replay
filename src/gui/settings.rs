//! The Settings tab: the memory ceiling, the Start-with-Windows entry and the manual
//! stop/start of the redirection.

use super::{
    layout::{self, BLACK, ExpandableGroup, Frame, INNER_WIDTH, INNER_X, LINE, RED, button, label},
    text,
};
use crate::{
    config::{Config, MAX_MEMORY_LIMIT_MB, MIN_MEMORY_LIMIT_MB},
    log,
    service::Status,
    sys::startup,
};
use winsafe::{co, gui, prelude::*};

/// Height the Startup group gains while it shows a Run-key error line.
const STARTUP_ERROR_EXTRA: i32 = LINE + 4;

#[derive(Clone)]
pub(super) struct SettingsPage {
    page: gui::TabPage,
    frames: Vec<Frame>,
    ceiling: gui::Edit,
    _ceiling_spin: gui::UpDown,
    pub(super) apply: gui::Button,
    settings_error: gui::Label,
    pub(super) start_with_windows: gui::CheckBox,
    /// The Startup group; it grows while the error label has something to show.
    startup: ExpandableGroup,
    recording_help: gui::Label,
    pub(super) recording: gui::Button,
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
    pub(super) fn new(
        parent: &(impl GuiParent + 'static),
        config: &Config,
        start_with_windows: bool,
    ) -> Self {
        let page = gui::TabPage::new(parent, gui::TabPageOpts::default());
        let storage = Self::storage_controls(&page, config);

        // Compact; the group adds a line for the error label when one occurs.
        let startup_group = Frame::new(&page, text::GROUP_STARTUP, 120, 78);
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

        let recording_group = Frame::new(&page, text::GROUP_RECORDING, 206, 90);
        let recording_help = label(&page, "", INNER_X, 226, INNER_WIDTH, 2);
        let recording = button(&page, text::STOP_AND_RESTORE, INNER_X, 264, 120);

        let startup = ExpandableGroup::new(
            startup_group.clone(),
            startup_error.clone(),
            STARTUP_ERROR_EXTRA,
        )
        .moving_frame(recording_group.clone())
        .moving([recording_help.clone()])
        .moving([recording.clone()]);
        let frames = vec![storage.group, startup_group, recording_group];

        // The two error lines are red; every other label is black.
        let errors = [storage.settings_error.clone(), startup_error];
        layout::draw(
            &page,
            page.on(),
            frames.clone(),
            co::COLOR::WINDOW,
            move |hwnd| {
                if errors.iter().any(|error| *error.hwnd() == *hwnd) {
                    RED
                } else {
                    BLACK
                }
            },
        );

        Self {
            page,
            frames,
            ceiling: storage.ceiling,
            _ceiling_spin: storage.ceiling_spin,
            apply: storage.apply,
            settings_error: storage.settings_error,
            start_with_windows,
            startup,
            recording_help,
            recording,
        }
    }

    fn storage_controls(page: &gui::TabPage, config: &Config) -> StorageControls {
        let group = Frame::new(page, text::GROUP_STORAGE, 6, 106);
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

    pub(super) fn page(&self) -> &gui::TabPage {
        &self.page
    }

    /// Finishes the parts that need the controls to exist.
    pub(super) fn on_create(&self) {
        layout::fit_titles(&self.frames);
        self.startup.on_create();
    }

    /// The extra height the page currently needs for its expanded group, in logical pixels.
    pub(super) fn extra(&self) -> i32 {
        self.startup.extra()
    }

    /// The configuration as currently entered, or the problem with it.
    pub(super) fn configuration(&self) -> Result<Config, String> {
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

    /// Shows the problem with the entered settings, or clears it.
    pub(super) fn show_settings_error(&self, message: &str) {
        layout::set_text(&self.settings_error, message);
    }

    /// Offers to stop or to start the redirection, whichever `status` allows.
    pub(super) fn update(&self, status: &Status) {
        let (recording_label, recording_help) = if status.mounted {
            (text::STOP_AND_RESTORE, text::STOP_HELP)
        } else {
            (text::RETRY_START, text::START_HELP)
        };
        layout::set_text(&self.recording_help, recording_help);
        layout::set_text(&self.recording, recording_label);
    }

    /// Writes the Start-with-Windows choice to the Run key, undoing the tick when that fails,
    /// and reports whether the Startup group changed height.
    pub(super) fn toggle_startup(&self) -> bool {
        let wanted = self.start_with_windows.is_checked();
        match startup::set(wanted) {
            Ok(()) => {
                log::info(if wanted {
                    "Start with Windows enabled"
                } else {
                    "Start with Windows disabled"
                });
                self.show_startup_error(None)
            }
            Err(error) => {
                let message = text::startup_setting_failed(&error);
                log::error(&message);
                self.start_with_windows.set_check(!wanted);
                self.show_startup_error(Some(&message))
            }
        }
    }

    /// The Startup group stays compact until a Run-key error needs its extra line; reports
    /// whether its height changed, so the caller refits the window.
    pub(super) fn show_startup_error(&self, message: Option<&str>) -> bool {
        self.startup.set_message(message)
    }
}
