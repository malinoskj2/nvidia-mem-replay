//! The Status tab: what the worker is doing, how much it has written, and where NVIDIA's
//! temporary files currently are.

use super::{
    layout::{
        self, AMBER, BLACK, ExpandableGroup, Frame, GRAY, GREEN, INNER_WIDTH, INNER_X, LINE, RED,
        TintedLabel, VALUE_WIDTH, VALUE_X, label, path_label,
    },
    text,
};
use crate::service::Status;
use winsafe::{co, gui, prelude::*};

/// Height the Status group gains while it shows warnings, notices or errors (two lines).
const NOTICE_EXTRA: i32 = 2 * LINE + 4;

#[derive(Clone)]
pub(super) struct StatusPage {
    page: gui::TabPage,
    frames: Vec<Frame>,
    /// The Status group; it grows while the notice label has something to show.
    status: ExpandableGroup,
    state: TintedLabel,
    written: gui::Label,
    buffer: gui::Label,
    notice: TintedLabel,
    path_state: TintedLabel,
    original: gui::Label,
    ram: gui::Label,
    ceiling: gui::Label,
}

impl StatusPage {
    pub(super) fn new(parent: &(impl GuiParent + 'static)) -> Self {
        let page = gui::TabPage::new(parent, gui::TabPageOpts::default());

        // Compact; the group adds two lines for the notice label when needed.
        let status_group = Frame::new(&page, text::GROUP_STATUS, 6, 86);
        let state = TintedLabel::new(&page, INNER_X, 26, INNER_WIDTH, 1, GRAY);
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
        let notice = TintedLabel::new(&page, INNER_X, 90, INNER_WIDTH, 2, AMBER);

        let locations_group = Frame::new(&page, text::GROUP_LOCATIONS, 100, 112);
        let path_state = TintedLabel::new(&page, INNER_X, 120, INNER_WIDTH, 1, GRAY);
        let original_caption = label(
            &page,
            text::ORIGINAL_LOCATION,
            INNER_X,
            142,
            VALUE_X - INNER_X,
            1,
        );
        let original = path_label(&page, VALUE_X, 142, VALUE_WIDTH);
        let ram_caption = label(
            &page,
            text::RAM_LOCATION,
            INNER_X,
            162,
            VALUE_X - INNER_X,
            1,
        );
        let ram = path_label(&page, VALUE_X, 162, VALUE_WIDTH);
        let ceiling_caption = label(
            &page,
            text::BUFFER_CEILING,
            INNER_X,
            182,
            VALUE_X - INNER_X,
            1,
        );
        let ceiling = label(&page, "", VALUE_X, 182, VALUE_WIDTH, 1);

        let status =
            ExpandableGroup::new(status_group.clone(), notice.label().clone(), NOTICE_EXTRA)
                .moving_frame(locations_group.clone())
                .moving([
                    path_state.label().clone(),
                    original_caption,
                    original.clone(),
                    ram_caption,
                    ram.clone(),
                    ceiling_caption,
                    ceiling.clone(),
                ]);
        let frames = vec![status_group, locations_group];

        // The three tinted lines read their colour here; every other label is black.
        let tints = [state.clone(), path_state.clone(), notice.clone()];
        layout::draw(
            &page,
            page.on(),
            frames.clone(),
            co::COLOR::WINDOW,
            move |hwnd| {
                tints
                    .iter()
                    .find_map(|tint| tint.colour_of(hwnd))
                    .unwrap_or(BLACK)
            },
        );

        Self {
            page,
            frames,
            status,
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

    pub(super) fn page(&self) -> &gui::TabPage {
        &self.page
    }

    /// Finishes the parts that need the controls to exist.
    pub(super) fn on_create(&self) {
        layout::fit_titles(&self.frames);
        self.status.on_create();
    }

    /// The extra height the page currently needs for its expanded group, in logical pixels.
    pub(super) fn extra(&self) -> i32 {
        self.status.extra()
    }

    /// Shows `status`, with `limit` as the buffer ceiling, and reports whether the Status
    /// group changed height.
    pub(super) fn update(&self, status: &Status, limit: u64) -> bool {
        self.state
            .set_colour(if status.active { GREEN } else { GRAY });
        self.path_state
            .set_colour(if status.mounted { GREEN } else { GRAY });
        self.notice
            .set_colour(if status.error.is_some() { RED } else { AMBER });

        self.state.set_text(&text::status(status.message.as_str()));
        layout::set_text(
            &self.written,
            &text::lifetime_written(status.lifetime_bytes),
        );
        layout::set_text(
            &self.buffer,
            &text::buffer_allocated(status.sample.as_ref().map(|sample| sample.buffer_bytes)),
        );
        let notices: Vec<&str> = [
            status.warning.as_deref().filter(|_| status.mounted),
            status.notice.as_deref(),
            status.error.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let notice = notices.join("\r\n");
        let grown = self
            .status
            .set_message((!notice.is_empty()).then_some(notice.as_str()));
        self.path_state.set_text(&text::status(if status.mounted {
            text::PATH_SWAPPED
        } else {
            text::PATH_ORIGINAL
        }));
        layout::set_text(
            &self.original,
            status.original_path.as_deref().unwrap_or("—"),
        );
        layout::set_text(&self.ram, status.target.as_deref().unwrap_or("—"));
        layout::set_text(&self.ceiling, &text::buffer_ceiling(limit));
        grown
    }
}
