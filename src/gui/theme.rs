//! Classic Windows dialog styling: the system dialog palette, Segoe UI at 9 pt and titled
//! group boxes, matching control-panel style applets rather than egui's defaults.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, RichText,
    Stroke, TextStyle, Visuals,
};
use std::{path::PathBuf, sync::Arc};

/// `SystemColors.Control`, the dialog background.
pub(super) const WINDOW_BACKGROUND: Color32 = Color32::from_rgb(0xF0, 0xF0, 0xF0);
pub(super) const TEXT: Color32 = Color32::BLACK;
/// `SystemColors.GrayText`.
pub(super) const GRAY_TEXT: Color32 = Color32::from_rgb(0x6D, 0x6D, 0x6D);
pub(super) const OK_TEXT: Color32 = Color32::from_rgb(0x10, 0x7C, 0x10);
pub(super) const WARNING_TEXT: Color32 = Color32::from_rgb(0x9D, 0x5D, 0x00);
pub(super) const ERROR_TEXT: Color32 = Color32::from_rgb(0xC4, 0x2B, 0x1C);
const FIELD_BACKGROUND: Color32 = Color32::WHITE;
const BUTTON_FACE: Color32 = Color32::from_rgb(0xE1, 0xE1, 0xE1);
const BUTTON_BORDER: Color32 = Color32::from_rgb(0xAD, 0xAD, 0xAD);
const BUTTON_HOVER: Color32 = Color32::from_rgb(0xE5, 0xF1, 0xFB);
const BUTTON_PRESSED: Color32 = Color32::from_rgb(0xCC, 0xE4, 0xF7);
const PRESSED_BORDER: Color32 = Color32::from_rgb(0x00, 0x54, 0x99);
/// `SystemColors.Highlight`.
const HIGHLIGHT: Color32 = Color32::from_rgb(0x00, 0x78, 0xD7);
const GROUP_BORDER: Color32 = Color32::from_rgb(0xDC, 0xDC, 0xDC);
const BORDER_WIDTH: f32 = 1.0;
/// The tab page, white like a themed Windows tab control.
const PAGE: Color32 = Color32::from_rgb(0xFC, 0xFC, 0xFC);
const TAB_BORDER: Color32 = Color32::from_rgb(0xD9, 0xD9, 0xD9);
const TAB_HEIGHT: f32 = 22.0;
const TAB_RAISE: f32 = 2.0;
const TAB_PADDING_X: f32 = 10.0;

/// Segoe UI 9 pt.
pub(super) const BODY: f32 = 12.0;
const SMALL: f32 = 11.0;
const FONTS_DIRECTORY_ENV: &str = "WINDIR";
const REGULAR_FONT: &str = "segoeui.ttf";
const BOLD_FONT: &str = "segoeuib.ttf";
const REGULAR_FAMILY: &str = "segoe";
const BOLD_FAMILY: &str = "segoe-bold";
const GROUP_TITLE_INSET: f32 = 8.0;
const GROUP_TITLE_PADDING: f32 = 3.0;

pub(super) fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_visuals(visuals());
    ctx.style_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(SMALL)),
            (TextStyle::Body, FontId::proportional(BODY)),
            (TextStyle::Button, FontId::proportional(BODY)),
            (TextStyle::Heading, bold(BODY)),
            (TextStyle::Monospace, FontId::monospace(BODY)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 4.0);
        style.spacing.window_margin = Margin::same(10);
    });
}

pub(super) fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD_FAMILY.into()))
}

/// A value shown next to its label, as dialogs show read-only fields.
pub(super) fn value(text: &str) -> RichText {
    RichText::new(text).font(bold(BODY))
}

/// Segoe UI from the Windows font directory, with egui's bundled fonts as the fallback.
fn fonts() -> FontDefinitions {
    let mut definitions = FontDefinitions::default();
    let directory = std::env::var_os(FONTS_DIRECTORY_ENV)
        .map(PathBuf::from)
        .map(|windows| windows.join("Fonts"));
    let mut bold_family = Vec::new();

    if let Some(directory) = &directory {
        if let Ok(bytes) = std::fs::read(directory.join(REGULAR_FONT)) {
            definitions.font_data.insert(
                REGULAR_FAMILY.to_owned(),
                Arc::new(FontData::from_owned(bytes)),
            );
            definitions
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, REGULAR_FAMILY.to_owned());
        }
        if let Ok(bytes) = std::fs::read(directory.join(BOLD_FONT)) {
            definitions.font_data.insert(
                BOLD_FAMILY.to_owned(),
                Arc::new(FontData::from_owned(bytes)),
            );
            bold_family.push(BOLD_FAMILY.to_owned());
        }
    }

    bold_family.extend(
        definitions
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    definitions
        .families
        .insert(FontFamily::Name(BOLD_FAMILY.into()), bold_family);
    definitions
}

fn visuals() -> Visuals {
    let mut visuals = Visuals::light();
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = WINDOW_BACKGROUND;
    visuals.window_fill = WINDOW_BACKGROUND;
    visuals.window_stroke = Stroke::new(BORDER_WIDTH, BUTTON_BORDER);
    visuals.extreme_bg_color = FIELD_BACKGROUND;
    visuals.faint_bg_color = Color32::from_rgb(0xE8, 0xE8, 0xE8);
    visuals.selection.bg_fill = HIGHLIGHT;
    visuals.selection.stroke = Stroke::new(BORDER_WIDTH, Color32::WHITE);

    let radius = CornerRadius::same(2);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = WINDOW_BACKGROUND;
    widgets.noninteractive.weak_bg_fill = WINDOW_BACKGROUND;
    widgets.noninteractive.bg_stroke = Stroke::new(BORDER_WIDTH, GROUP_BORDER);
    widgets.noninteractive.fg_stroke = Stroke::new(BORDER_WIDTH, TEXT);
    widgets.noninteractive.corner_radius = radius;
    for (state, fill, border) in [
        (&mut widgets.inactive, BUTTON_FACE, BUTTON_BORDER),
        (&mut widgets.hovered, BUTTON_HOVER, HIGHLIGHT),
        (&mut widgets.active, BUTTON_PRESSED, PRESSED_BORDER),
        (&mut widgets.open, BUTTON_HOVER, HIGHLIGHT),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(BORDER_WIDTH, border);
        state.fg_stroke = Stroke::new(BORDER_WIDTH, TEXT);
        state.corner_radius = radius;
        state.expansion = 0.0;
    }

    visuals
}

/// Styles the widgets added to `ui` as Windows edit fields: white, gray border, square.
pub(super) fn field_style(ui: &mut egui::Ui) {
    const FIELD_BORDER: Color32 = Color32::from_rgb(0x7A, 0x7A, 0x7A);
    let widgets = &mut ui.style_mut().visuals.widgets;
    for (state, border) in [
        (&mut widgets.inactive, FIELD_BORDER),
        (&mut widgets.hovered, HIGHLIGHT),
        (&mut widgets.active, PRESSED_BORDER),
        (&mut widgets.open, HIGHLIGHT),
    ] {
        state.bg_fill = FIELD_BACKGROUND;
        state.weak_bg_fill = FIELD_BACKGROUND;
        state.bg_stroke = Stroke::new(BORDER_WIDTH, border);
        state.corner_radius = CornerRadius::ZERO;
    }
    ui.style_mut().spacing.button_padding = egui::vec2(6.0, 3.0);
}

/// A titled group box: an etched frame whose title sits on the top border.
pub(super) fn group_box<R>(
    ui: &mut egui::Ui,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let outer_top = 7;
    let frame = egui::Frame::new()
        .stroke(Stroke::new(BORDER_WIDTH, GROUP_BORDER))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin {
            left: 10,
            right: 10,
            top: 12,
            bottom: 10,
        })
        .outer_margin(Margin {
            left: 0,
            right: 0,
            top: outer_top,
            bottom: 4,
        });

    let shown = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        add_contents(ui)
    });

    let border_top = shown.response.rect.top() + f32::from(outer_top);
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(title.to_owned(), FontId::proportional(BODY), TEXT);
    let position = egui::pos2(
        shown.response.rect.left() + GROUP_TITLE_INSET,
        border_top - galley.size().y / 2.0,
    );
    let backing = egui::Rect::from_min_size(
        egui::pos2(position.x - GROUP_TITLE_PADDING, position.y),
        galley.size() + egui::vec2(2.0 * GROUP_TITLE_PADDING, 0.0),
    );
    // The title sits on the border, over whatever surface the box is drawn on.
    painter.rect_filled(backing, CornerRadius::ZERO, ui.visuals().panel_fill);
    painter.galley(position, galley, TEXT);

    shown.inner
}

/// A row of Windows-style tabs; returns the selected tab's rectangle for the page below.
pub(super) fn tab_strip<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    tabs: &[(T, &str)],
    selected: &mut T,
) -> Option<egui::Rect> {
    let mut selected_rect = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for &(value, label) in tabs {
            let active = value == *selected;
            let galley =
                ui.painter()
                    .layout_no_wrap(label.to_owned(), FontId::proportional(BODY), TEXT);
            let size = egui::vec2(
                galley.size().x + 2.0 * TAB_PADDING_X,
                TAB_HEIGHT + TAB_RAISE,
            );
            let (allocated, response) = ui.allocate_exact_size(size, egui::Sense::click());
            if response.clicked() {
                *selected = value;
            }

            // Inactive tabs sit lower than the selected one, which joins the page.
            let rect = if active {
                allocated
            } else {
                egui::Rect::from_min_max(
                    egui::pos2(allocated.min.x, allocated.min.y + TAB_RAISE),
                    allocated.max,
                )
            };
            let fill = if active {
                PAGE
            } else if response.hovered() {
                BUTTON_HOVER
            } else {
                WINDOW_BACKGROUND
            };
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::ZERO, fill);
            let stroke = Stroke::new(BORDER_WIDTH, TAB_BORDER);
            painter.line_segment([rect.left_bottom(), rect.left_top()], stroke);
            painter.line_segment([rect.left_top(), rect.right_top()], stroke);
            painter.line_segment([rect.right_top(), rect.right_bottom()], stroke);
            painter.galley(
                egui::pos2(
                    rect.center().x - galley.size().x / 2.0,
                    rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                TEXT,
            );
            if active {
                selected_rect = Some(rect);
            }
        }
    });

    selected_rect
}

/// The page under the tab strip, joined to the selected tab.
pub(super) fn tab_page<R>(
    ui: &mut egui::Ui,
    selected_tab: Option<egui::Rect>,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.add_space(-ui.spacing().item_spacing.y);
    let shown = egui::Frame::new()
        .fill(PAGE)
        .stroke(Stroke::new(BORDER_WIDTH, TAB_BORDER))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.style_mut().visuals.panel_fill = PAGE;
            ui.set_width(ui.available_width());
            add_contents(ui)
        });

    if let Some(tab) = selected_tab {
        let top = shown.response.rect.top();
        let opening = egui::Rect::from_min_max(
            egui::pos2(tab.left() + BORDER_WIDTH, top),
            egui::pos2(tab.right() - BORDER_WIDTH, top + BORDER_WIDTH),
        );
        ui.painter().rect_filled(opening, CornerRadius::ZERO, PAGE);
    }

    shown.inner
}
