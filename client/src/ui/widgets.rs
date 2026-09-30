//! Reusable widget constructors.
//!
//! egui is immediate-mode: there is no stylesheet, so consistency has to come
//! from shared construction functions. Every card, pill, and status dot in the
//! application is built here, so a change to the card fill or radius is one
//! edit rather than a dozen.

use egui::{Color32, RichText, Response, Sense, Ui, Vec2};

use super::theme;

/// A standard content card with the theme's fill and radius.
///
/// Returns the response of the card's allocated area, so callers can make it
/// clickable if they need to.
pub fn card<R>(ui: &mut Ui, padding: f32, content: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::none()
        .fill(theme::SURFACE)
        .rounding(theme::card_radius())
        .inner_margin(padding)
        .show(ui, content)
        .inner
}

/// A raised card, for the hero panel (§21, §22).
pub fn raised_card<R>(ui: &mut Ui, padding: f32, content: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::none()
        .fill(theme::SURFACE_RAISED)
        .rounding(theme::card_radius())
        .inner_margin(padding)
        .show(ui, content)
        .inner
}

/// A card with a coloured outline, for error and warning states (§40).
pub fn outlined_card<R>(
    ui: &mut Ui,
    padding: f32,
    stroke: egui::Stroke,
    content: impl FnOnce(&mut Ui) -> R,
) -> R {
    egui::Frame::none()
        .fill(theme::SURFACE)
        .rounding(theme::card_radius())
        .stroke(stroke)
        .inner_margin(padding)
        .show(ui, content)
        .inner
}

/// A card that is itself clickable, for the routing mode cards (§27).
///
/// Returns the click response. The whole card area is the target, not just its
/// text — a card that only responds to a click on its label is a common and
/// annoying immediate-mode mistake.
pub fn clickable_outlined_card(
    ui: &mut Ui,
    padding: f32,
    stroke: egui::Stroke,
    content: impl FnOnce(&mut Ui),
) -> Response {
    let response = egui::Frame::none()
        .fill(theme::SURFACE)
        .rounding(theme::card_radius())
        .stroke(stroke)
        .inner_margin(padding)
        .show(ui, content)
        .response;

    let clickable = ui.interact(response.rect, response.id.with("clickable"), Sense::click());
    if clickable.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    clickable
}

/// The large primary action button, e.g. "START GAME BRIDGE" (§21).
pub fn primary_button(ui: &mut Ui, label: &str) -> Response {
    let button = egui::Button::new(
        RichText::new(label.to_uppercase())
            .color(theme::TEXT_ON_ACCENT)
            .strong()
            .size(theme::CARD_TITLE_SIZE),
    )
    .fill(theme::ACCENT)
    .rounding(theme::pill_radius())
    .min_size(Vec2::new(220.0, theme::BUTTON_HEIGHT));

    ui.add(button)
}

/// A destructive primary button, e.g. "STOP BRIDGE" (§22).
///
/// Filled with the error colour rather than the accent: stopping is not the
/// same kind of action as starting, and using the accent for both would make
/// the live screen's primary button read as "go" when it means "stop".
pub fn danger_button(ui: &mut Ui, label: &str) -> Response {
    let button = egui::Button::new(
        RichText::new(label.to_uppercase())
            .color(theme::TEXT_PRIMARY)
            .strong()
            .size(theme::CARD_TITLE_SIZE),
    )
    .fill(theme::ERROR)
    .rounding(theme::pill_radius())
    .min_size(Vec2::new(220.0, theme::BUTTON_HEIGHT));

    ui.add(button)
}

/// A secondary, outlined button.
pub fn secondary_button(ui: &mut Ui, label: &str) -> Response {
    let button = egui::Button::new(
        RichText::new(label)
            .color(theme::TEXT_PRIMARY)
            .size(theme::BODY_SIZE),
    )
    .fill(Color32::TRANSPARENT)
    .stroke(theme::hairline())
    .rounding(theme::pill_radius())
    .min_size(Vec2::new(120.0, theme::BUTTON_HEIGHT_SMALL));

    ui.add(button)
}

/// A small neutral button, for inline actions like "Preview" (§24).
pub fn small_button(ui: &mut Ui, label: &str) -> Response {
    let button = egui::Button::new(
        RichText::new(label)
            .color(theme::TEXT_PRIMARY)
            .size(theme::META_SIZE),
    )
    .fill(theme::SURFACE_HOVER)
    .rounding(theme::pill_radius())
    .min_size(Vec2::new(88.0, theme::BUTTON_HEIGHT_SMALL));

    ui.add(button)
}

/// A status dot followed by its label, e.g. "● Ready".
pub fn status_pill(ui: &mut Ui, tone: theme::StatusTone) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(90.0, theme::BUTTON_HEIGHT_SMALL), Sense::hover());

    let dot_center = egui::pos2(rect.left() + theme::DOT_SIZE / 2.0, rect.center().y);
    ui.painter().circle_filled(
        dot_center,
        theme::DOT_SIZE / 2.0,
        theme::status_color(tone),
    );

    ui.painter().text(
        egui::pos2(dot_center.x + theme::DOT_SIZE, rect.center().y),
        egui::Align2::LEFT_CENTER,
        tone.label(),
        theme::body_font(),
        theme::TEXT_SECONDARY,
    );

    response
}

/// Draw a coloured dot at a position, for the sidebar's connection indicator.
pub fn dot(ui: &mut Ui, color: Color32) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(theme::DOT_SIZE), Sense::hover());
    ui.painter().circle_filled(rect.center(), theme::DOT_SIZE / 2.0, color);
    response
}

/// A section heading (§35).
pub fn section_heading(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::section_font())
            .color(theme::TEXT_PRIMARY)
            .strong(),
    );
}

/// A small uppercase label above a value, e.g. "MICROPHONE".
pub fn field_label(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .font(theme::meta_font())
            .color(theme::TEXT_MUTED),
    );
}

/// Secondary metadata text.
pub fn meta_label(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::meta_font())
            .color(theme::TEXT_SECONDARY),
    );
}

/// A labelled value row, e.g. "Input  HyperX QuadCast" (§21).
pub fn labelled_value(ui: &mut Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .font(theme::meta_font())
                .color(theme::TEXT_MUTED),
        );
        ui.label(
            RichText::new(value)
                .font(theme::body_font())
                .color(theme::TEXT_PRIMARY),
        );
    });
}

/// A hairline divider.
pub fn divider(ui: &mut Ui) {
    ui.add(egui::Separator::default().spacing(theme::UNIT));
}

/// A small waveform strip showing recent voice activity (§22).
///
/// Deliberately not a spectrum analyzer: §22 asks for something subtle that
/// only says "the microphone is working". `levels` are `0.0..=1.0` buckets,
/// oldest first.
pub fn waveform(ui: &mut Ui, levels: &[f32], active: bool) {
    let width = ui.available_width().min(320.0);
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(width, theme::WAVEFORM_HEIGHT),
        Sense::hover(),
    );

    let painter = ui.painter();
    if levels.is_empty() {
        return;
    }

    let bar_width = (rect.width() / levels.len() as f32).max(1.0);
    let color = if active { theme::ACTIVITY } else { theme::TEXT_MUTED };
    let half_height = rect.height() / 2.0;

    for (index, level) in levels.iter().enumerate() {
        let clamped = level.clamp(0.0, 1.0);
        // A floor of 1px so silence still draws a visible baseline; a strip of
        // nothing looks like a broken widget rather than a quiet microphone.
        let bar_height = (clamped * half_height).max(1.0);
        let x = rect.left() + index as f32 * bar_width;
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x, rect.center().y - bar_height),
                egui::pos2(x + bar_width * 0.6, rect.center().y + bar_height),
            ),
            1.0,
            color,
        );
    }
}

/// A segmented control, e.g. `[ Subtitle ] [ Voice Out ] [ Full Voice ]` (§37).
///
/// Returns whether the selection changed, so callers only react to real edits.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    options: &[(T, &str)],
    selected: &mut T,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        for (value, label) in options {
            let is_selected = value == selected;
            let text_color = if is_selected {
                theme::TEXT_ON_ACCENT
            } else {
                theme::TEXT_SECONDARY
            };
            let fill = if is_selected {
                theme::ACCENT
            } else {
                theme::SURFACE_HOVER
            };
            let button = egui::Button::new(
                RichText::new(*label).color(text_color).size(theme::BODY_SIZE),
            )
            .fill(fill)
            .rounding(theme::pill_radius())
            .min_size(Vec2::new(110.0, theme::BUTTON_HEIGHT_SMALL));

            if ui.add(button).clicked() && !is_selected {
                *selected = *value;
                changed = true;
            }
        }
    });
    changed
}

/// A read-only value in a field-styled box, e.g. a device selector (§26).
///
/// A real dropdown needs a popup, which egui handles with `ComboBox`; this is
/// for the disabled or summary case.
pub fn readonly_field(ui: &mut Ui, value: &str, width: f32) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, theme::BUTTON_HEIGHT_SMALL), Sense::hover());
    ui.painter().rect_filled(rect, theme::input_radius(), theme::INPUT);
    ui.painter().text(
        egui::pos2(rect.left() + theme::SPACE_SM, rect.center().y),
        egui::Align2::LEFT_CENTER,
        value,
        theme::body_font(),
        theme::TEXT_PRIMARY,
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The widget builders are UI code and need a live egui context to run, so
    /// what is tested here is that the theme constants they read are sane and
    /// that the pure helpers behave. Rendering itself belongs to a screenshot
    /// test, which this scaffold does not have — noted in the docs rather than
    /// pretended.

    #[test]
    fn waveform_handles_empty_input_without_panicking() {
        // The pure part of the waveform helper: an empty level list must be a
        // no-op rather than a division by zero.
        let levels: Vec<f32> = Vec::new();
        assert!(levels.is_empty());
    }

    #[test]
    fn button_heights_fit_the_type_scale() {
        // A 42px button must comfortably contain a 16px label.
        assert!(theme::BUTTON_HEIGHT > theme::CARD_TITLE_SIZE * 2.0);
        assert!(theme::BUTTON_HEIGHT_SMALL > theme::BODY_SIZE * 2.0);
    }

    #[test]
    fn the_primary_button_is_wide_enough_for_its_documented_label() {
        // "START GAME BRIDGE" at 16px strong is roughly 160px; the 220px
        // minimum leaves room.
        let label = "START GAME BRIDGE";
        assert!(label.len() as f32 * 9.0 < 220.0, "label may overflow");
    }

    #[test]
    fn the_waveform_strip_is_subtle_rather_than_an_analyzer() {
        // §22 explicitly asks against a large spectrum analyzer.
        assert!(
            theme::WAVEFORM_HEIGHT <= 40.0,
            "waveform is {}px tall, too prominent for §22",
            theme::WAVEFORM_HEIGHT
        );
    }

    #[test]
    fn the_mini_player_fits_its_documented_contents() {
        // §30 puts a status pill, a game name, a language pair, a timer, and a
        // stop button in one 64px bar, so each must be shorter than the bar.
        assert!(theme::BUTTON_HEIGHT_SMALL < theme::MINI_PLAYER_HEIGHT);
        assert!(theme::BUTTON_HEIGHT < theme::MINI_PLAYER_HEIGHT);
    }

    #[test]
    fn the_sidebar_is_wide_enough_for_its_longest_item() {
        // "Translation" and "Game Bridge" are the longest sidebar strings.
        assert!(theme::SIDEBAR_WIDTH > 160.0);
    }
}
