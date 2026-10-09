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
    painter.rect_filled(backing, CornerRadius::ZERO, WINDOW_BACKGROUND);
    painter.galley(position, galley, TEXT);

    shown.inner
}
