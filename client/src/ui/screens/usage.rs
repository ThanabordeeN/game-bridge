//! Usage screen: session history, modelled on Spotify's listening history
//! (§28).

use egui::RichText;

use crate::session::usage::UsageSummary;

use super::super::{app::App, theme, widgets};

/// Draw the Usage screen.
pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    widgets::section_heading(ui, "Usage");
    ui.add_space(theme::SPACE_SM);

    if app.usage_history.is_empty() {
        empty_state(ui);
        return;
    }

    today_section(ui, app);
    ui.add_space(theme::SPACE_LG);
    summary_section(ui, app);
    ui.add_space(theme::SPACE_LG);
    all_sessions(ui, app);
}

fn empty_state(ui: &mut egui::Ui) {
    widgets::card(ui, theme::SPACE_XL, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(theme::SPACE_MD);
            ui.label(
                RichText::new("No sessions yet")
                    .font(theme::section_font())
                    .color(theme::TEXT_PRIMARY),
            );
            ui.add_space(theme::SPACE_XS);
            ui.label(
                RichText::new(
                    "Once you start a bridge, each session's active voice time and cost appear \
                     here.",
                )
                .font(theme::body_font())
                .color(theme::TEXT_SECONDARY),
            );
            ui.add_space(theme::SPACE_MD);
        });
    });
}

/// Today's sessions, in the row format §28 shows.
fn today_section(ui: &mut egui::Ui, app: &App) {
    widgets::field_label(ui, "Today");
    ui.add_space(theme::SPACE_SM);

    let now = crate::network::auth::now_unix();
    let today: Vec<_> = app
        .usage_history
        .since(now, 24 * 3600)
        .into_iter()
        .rev()
        .collect();

    let records: Vec<_> = if today.is_empty() {
        app.usage_history.records().iter().rev().take(5).collect()
    } else {
        today
    };

    widgets::card(ui, theme::SPACE_MD, |ui| {
        for (index, record) in records.iter().enumerate() {
            if index > 0 {
                ui.add_space(theme::SPACE_SM);
                widgets::divider(ui);
                ui.add_space(theme::SPACE_SM);
            }

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(&record.application)
                            .font(theme::card_title_font())
                            .color(theme::TEXT_PRIMARY)
                            .strong(),
                    );
                    // Two lines, per §28: session length and active minutes are
                    // different numbers and the screen exists to show that.
                    ui.label(
                        RichText::new(format!(
                            "{} session",
                            record.session_duration_display()
                        ))
                        .font(theme::meta_font())
                        .color(theme::TEXT_SECONDARY),
                    );
                    ui.label(
                        RichText::new(record.active_minutes_display())
                            .font(theme::meta_font())
                            .color(theme::TEXT_MUTED),
                    );
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(record.cost_display())
                            .font(theme::body_font())
                            .color(theme::ACCENT),
                    );
                });
            });
        }
    });
}

/// The month summary (§28).
fn summary_section(ui: &mut egui::Ui, app: &App) {
    let summary = UsageSummary::from_records(app.usage_history.records());

    widgets::field_label(ui, "Summary");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        summary_row(ui, "Gameplay", &summary.session_display());
        ui.add_space(theme::SPACE_XS);
        summary_row(ui, "Active Voice", &summary.speech_display());
        ui.add_space(theme::SPACE_XS);
        summary_row(ui, "Credits Used", &summary.cost_display());
        ui.add_space(theme::SPACE_SM);

        if summary.sessions > 0 {
            widgets::meta_label(
                ui,
                &format!(
                    "{} across {} session{}",
                    format_percent(summary.duty_cycle()),
                    summary.sessions,
                    if summary.sessions == 1 { "" } else { "s" }
                ),
            );
        }
    });
}

fn summary_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .font(theme::body_font())
                .color(theme::TEXT_SECONDARY),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(value)
                    .font(theme::body_font())
                    .color(theme::TEXT_PRIMARY),
            );
        });
    });
}

fn format_percent(fraction: f32) -> String {
    format!("{:.0}% of that time was speech", fraction * 100.0)
}

/// Every retained session.
fn all_sessions(ui: &mut egui::Ui, app: &App) {
    widgets::field_label(ui, "All Sessions");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        for record in app.usage_history.records().iter().rev() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&record.application)
                        .font(theme::body_font())
                        .color(theme::TEXT_PRIMARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(record.cost_display())
                            .font(theme::meta_font())
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.add_space(theme::SPACE_MD);
                    ui.label(
                        RichText::new(record.active_minutes_display())
                            .font(theme::meta_font())
                            .color(theme::TEXT_MUTED),
                    );
                });
            });
            ui.add_space(theme::SPACE_XS);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::usage::UsageRecord;
    use game_bridge_protocol::mode::RoutingMode;
    use game_bridge_protocol::usage::{ActiveVoice, ServiceTier};

    fn record(session_ms: u64, speech_ms: u64, cost_minor: i64) -> UsageRecord {
        let mut active_voice = ActiveVoice::default();
        active_voice.record_elapsed(session_ms);
        if speech_ms > 0 {
            active_voice.record_speech(speech_ms);
        }
        UsageRecord {
            application: "Valorant".into(),
            mode: RoutingMode::VoiceOut,
            tier: ServiceTier::Voice,
            active_voice,
            cost_minor,
            currency: "THB".into(),
            started_at_unix: 1_700_000_000,
        }
    }

    #[test]
    fn the_summary_uses_the_documented_row_labels() {
        // §28 shows these three rows.
        let records = vec![record(60_000, 30_000, 125)];
        let summary = UsageSummary::from_records(&records);
        assert_eq!(summary.session_display(), "1 min");
        assert_eq!(summary.speech_display(), "0 min");
        assert_eq!(summary.cost_display(), "฿1");
    }

    #[test]
    fn the_duty_cycle_line_reads_naturally() {
        assert_eq!(format_percent(0.25), "25% of that time was speech");
        assert_eq!(format_percent(0.0), "0% of that time was speech");
    }

    #[test]
    fn a_session_with_no_speech_shows_zero_active_time() {
        // The distinction the screen exists to make.
        let r = record(3600_000, 0, 0);
        // Exactly one hour renders in the `Hh MMm` form, consistent with the
        // `1h 08m` example in §28.
        assert_eq!(r.session_duration_display(), "1h 00m");
        assert_eq!(r.active_minutes_display(), "0.0 active min");
        assert_eq!(r.cost_display(), "฿0.00");
    }

    #[test]
    fn an_empty_history_renders_the_empty_state() {
        let app = App::default();
        assert!(app.usage_history.is_empty());
        let summary = UsageSummary::from_records(app.usage_history.records());
        assert_eq!(summary.sessions, 0);
    }
}
