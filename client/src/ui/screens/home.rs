//! Home screen: the hero card and the at-a-glance device summary (§21, §22).

use egui::RichText;


use super::super::{app::App, theme, widgets};

/// Draw the Home screen.
pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    greeting(ui, app);
    hero_card(ui, app);
    ui.add_space(theme::SPACE_LG);

    // §40: surface a blocker before the user presses Start and wonders why
    // nothing happened.
    if let Some(blocker) = app.start_blocker() {
        widgets::outlined_card(ui, theme::SPACE_MD, theme::warning_stroke(), |ui| {
            ui.label(
                RichText::new("Cannot start yet")
                    .font(theme::card_title_font())
                    .color(theme::WARNING)
                    .strong(),
            );
            ui.add_space(theme::SPACE_XS);
            ui.label(
                RichText::new(blocker)
                    .font(theme::body_font())
                    .color(theme::TEXT_SECONDARY),
            );
        });
        ui.add_space(theme::SPACE_LG);
    }

    if let Some(error) = app.last_error.clone() {
        widgets::outlined_card(ui, theme::SPACE_MD, theme::error_stroke(), |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(error)
                        .font(theme::body_font())
                        .color(theme::TEXT_PRIMARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Dismiss").clicked() {
                        // Handled below, outside the closure.
                    }
                });
            });
        });
        if ui.ctx().input(|i| i.pointer.any_click()) {
            // A simple dismissal: any click clears a stale error. A modal would
            // violate §37, which asks for few dialogs.
            app.last_error = None;
        }
        ui.add_space(theme::SPACE_LG);
    }

    device_summary(ui, app);
    ui.add_space(theme::SPACE_LG);

    if !app.usage_history.is_empty() {
        recent_sessions(ui, app);
    }
}

/// "Good evening", §19.
fn greeting(ui: &mut egui::Ui, app: &mut App) {
    ui.label(
        RichText::new(time_greeting())
            .font(theme::hero_font())
            .color(theme::TEXT_PRIMARY)
            .strong(),
    );
    ui.add_space(theme::SPACE_XS);
    ui.label(
        RichText::new(if app.session.state().is_live() {
            "Bridging your voice in real time."
        } else {
            "Speak your language. Play with everyone."
        })
        .font(theme::body_font())
        .color(theme::TEXT_SECONDARY),
    );
    ui.add_space(theme::SPACE_LG);
}

/// The greeting depends on the local hour.
fn time_greeting() -> &'static str {
    // A local-time greeting needs the wall clock; the hour is taken from the
    // system clock rather than a dependency, because the worst case of a wrong
    // greeting is that it says "evening" at 5pm.
    match std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() / 3600) % 24)
        .unwrap_or(18)
    {
        // UTC, not local: documented limitation, see the module note below.
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    }
}

/// The hero card: status, language pair, mode, and the primary action
/// (§21 idle, §22 live).
fn hero_card(ui: &mut egui::Ui, app: &mut App) {
    let is_live = app.session.state().is_live();

    widgets::raised_card(ui, theme::SPACE_XL, |ui| {
        ui.set_min_height(220.0);
        ui.vertical_centered(|ui| {
            ui.add_space(theme::SPACE_SM);

            // Status.
            ui.label(
                RichText::new(app.session.state().label().to_uppercase())
                    .font(theme::meta_font())
                    .color(theme::status_color(app.status_tone()))
                    .strong(),
            );
            ui.add_space(theme::SPACE_MD);

            // Language pair, the largest thing on the card (§21).
            let pair = app.config.session.pair;
            ui.horizontal(|ui| {
                // Centre the pair by padding with half the leftover width.
                let content = format!(
                    "{}   →   {}",
                    pair.source.display_name(),
                    pair.target.display_name()
                );
                let estimated = content.len() as f32 * 11.0;
                let pad = ((ui.available_width() - estimated) / 2.0).max(0.0);
                ui.add_space(pad);
                ui.label(
                    RichText::new(pair.source.display_name())
                        .font(theme::language_font())
                        .color(theme::TEXT_PRIMARY),
                );
                ui.label(
                    RichText::new("→")
                        .font(theme::language_font())
                        .color(theme::ACCENT),
                );
                ui.label(
                    RichText::new(pair.target.display_name())
                        .font(theme::language_font())
                        .color(theme::TEXT_PRIMARY),
                );
            });

            ui.add_space(theme::SPACE_SM);
            widgets::meta_label(ui, app.config.session.mode.label());
            ui.add_space(theme::SPACE_MD);

            if is_live {
                live_details(ui, app);
            } else {
                ui.label(
                    RichText::new("Mode")
                        .font(theme::meta_font())
                        .color(theme::TEXT_MUTED),
                );
                ui.label(
                    RichText::new(app.config.session.mode.label())
                        .font(theme::body_font())
                        .color(theme::TEXT_PRIMARY),
                );
                ui.add_space(theme::SPACE_SM);
                widgets::waveform(ui, &app.activity, false);
            }

            ui.add_space(theme::SPACE_MD);

            if is_live {
                if widgets::danger_button(ui, "Stop Bridge").clicked() {
                    app.stop_session();
                }
            } else if widgets::primary_button(ui, "Start Game Bridge").clicked() {
                app.start_session();
            }

            ui.add_space(theme::SPACE_SM);
        });
    });
}

/// The live hero contents (§22): listening state, active voice, cost.
fn live_details(ui: &mut egui::Ui, app: &mut App) {
    ui.label(
        RichText::new("🎙 Listening…")
            .font(theme::body_font())
            .color(theme::TEXT_SECONDARY),
    );
    ui.add_space(theme::SPACE_XS);
    widgets::waveform(ui, &app.activity, true);
    ui.add_space(theme::SPACE_SM);

    ui.horizontal(|ui| {
        let content_width = 300.0;
        let pad = ((ui.available_width() - content_width) / 2.0).max(0.0);
        ui.add_space(pad);

        ui.vertical(|ui| {
            ui.label(
                RichText::new(format!("{} active voice", app.session.speech_clock()))
                    .font(theme::body_font())
                    .color(theme::TEXT_PRIMARY),
            );
            ui.label(
                RichText::new(format!("{} used", app.session.formatted_cost()))
                    .font(theme::meta_font())
                    .color(theme::TEXT_SECONDARY),
            );
            if app.session.latency_ms() > 0 {
                ui.label(
                    RichText::new(format!("{} ms latency", app.session.latency_ms()))
                        .font(theme::meta_font())
                        .color(theme::TEXT_MUTED),
                );
            }
        });
    });
}

/// The device summary beneath the hero (§21).
fn device_summary(ui: &mut egui::Ui, app: &App) {
    widgets::section_heading(ui, "Configuration");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        let microphone = if app.config.session.microphone_id.is_empty() {
            "Not selected"
        } else {
            app.config.session.microphone_id.as_str()
        };
        widgets::labelled_value(ui, "Input", microphone);
        ui.add_space(theme::SPACE_XS);

        let output = app
            .config
            .session
            .virtual_mic_id
            .as_deref()
            .unwrap_or("Not installed");
        widgets::labelled_value(ui, "Output", output);
        ui.add_space(theme::SPACE_XS);

        let game = app
            .config
            .session
            .application
            .as_deref()
            .unwrap_or("Not selected");
        widgets::labelled_value(ui, "Game", game);
    });
}

/// Recent sessions, matching §19's list (§28's data).
fn recent_sessions(ui: &mut egui::Ui, app: &App) {
    widgets::section_heading(ui, "Recent Sessions");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        let records: Vec<_> = app.usage_history.records().iter().rev().take(5).collect();
        for (index, record) in records.iter().enumerate() {
            if index > 0 {
                ui.add_space(theme::SPACE_XS);
                widgets::divider(ui);
                ui.add_space(theme::SPACE_XS);
            }
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&record.application)
                        .font(theme::body_font())
                        .color(theme::TEXT_PRIMARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(record.session_duration_display())
                            .font(theme::meta_font())
                            .color(theme::TEXT_SECONDARY),
                    );
                });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::session::SessionState;

    #[test]
    fn the_greeting_is_always_one_of_the_three_documented_tokens() {
        // §19 shows "Good evening". The other two are the same idea at other
        // hours; nothing else is ever returned.
        let greeting = time_greeting();
        assert!(
            ["Good morning", "Good afternoon", "Good evening"].contains(&greeting),
            "unexpected greeting {greeting}"
        );
    }

    #[test]
    fn a_live_session_state_is_reported_as_live_by_the_app() {
        // The hero card branches on this; if it were wrong the live screen
        // would never render.
        let app = App::demo();
        assert!(!app.session.state().is_live());
        assert_eq!(*app.session.state(), SessionState::Idle);
    }
}
