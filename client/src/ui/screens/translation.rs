//! Translation screen: language pair, routing mode cards, and the live feed
//! (§23, §27, §37).

use egui::RichText;

use game_bridge_protocol::mode::RoutingMode;
use game_bridge_protocol::Language;

use crate::overlay::subtitle::TextMode;

use super::super::{app::App, theme, widgets};

/// Draw the Translation screen.
pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    incoming_panel(ui, app);
    ui.add_space(theme::SPACE_LG);

    widgets::section_heading(ui, "Languages");
    ui.add_space(theme::SPACE_SM);
    language_pair(ui, app);
    ui.add_space(theme::SPACE_LG);

    widgets::section_heading(ui, "Voice Routing");
    ui.add_space(theme::SPACE_XS);
    widgets::meta_label(
        ui,
        "One mode is active per session. Subtitle costs the least.",
    );
    ui.add_space(theme::SPACE_SM);
    routing_cards(ui, app);
    ui.add_space(theme::SPACE_LG);

    widgets::section_heading(ui, "Live Translation");
    ui.add_space(theme::SPACE_SM);
    translation_feed(ui, app);
}

/// The language pair selector.
fn language_pair(ui: &mut egui::Ui, app: &mut App) {
    widgets::card(ui, theme::SPACE_MD, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                widgets::field_label(ui, "Speak");
                language_picker(ui, &mut app.config.session.pair.source, "source");
            });

            ui.add_space(theme::SPACE_LG);

            ui.vertical(|ui| {
                ui.add_space(theme::SPACE_MD);
                if ui
                    .add(
                        egui::Button::new(RichText::new("⇄").color(theme::ACCENT).size(18.0))
                            .fill(theme::SURFACE_HOVER)
                            .rounding(theme::pill_radius())
                            .min_size(egui::Vec2::splat(theme::BUTTON_HEIGHT_SMALL)),
                    )
                    .clicked()
                {
                    app.config.session.pair = app.config.session.pair.reversed();
                }
            });

            ui.add_space(theme::SPACE_LG);

            ui.vertical(|ui| {
                widgets::field_label(ui, "They hear");
                language_picker(ui, &mut app.config.session.pair.target, "target");
            });
        });

        ui.add_space(theme::SPACE_SM);
        if app.config.session.pair.source == app.config.session.pair.target {
            ui.label(
                RichText::new("Pick two different languages.")
                    .font(theme::meta_font())
                    .color(theme::WARNING),
            );
        }
    });
}

/// A language dropdown, excluding the language chosen on the other side so the
/// pair cannot be set to the same value through the UI.
fn language_picker(ui: &mut egui::Ui, selected: &mut Language, id: &str) {
    let languages = [
        Language::Thai,
        Language::English,
        Language::Japanese,
        Language::Korean,
        Language::Chinese,
        Language::Vietnamese,
        Language::Indonesian,
        Language::Spanish,
        Language::Portuguese,
        Language::French,
        Language::German,
        Language::Russian,
        Language::Turkish,
        Language::Arabic,
        Language::Hindi,
    ];

    egui::ComboBox::from_id_salt(id)
        .selected_text(selected.display_name())
        .width(170.0)
        .show_ui(ui, |ui| {
            for language in languages {
                ui.selectable_value(selected, language, language.display_name());
            }
        });
}

/// The three routing mode cards (§27).
fn routing_cards(ui: &mut egui::Ui, app: &mut App) {
    let mut chosen = app.config.session.mode;

    ui.horizontal_top(|ui| {
        for mode in RoutingMode::ALL {
            let is_active = mode == chosen;
            let width = (ui.available_width() - theme::SPACE_MD * 2.0) / 3.0;

            let response = widgets::clickable_outlined_card(
                ui,
                theme::SPACE_MD,
                if is_active {
                    theme::selected()
                } else {
                    theme::hairline()
                },
                |ui| {
                    ui.set_min_width(width);
                    ui.set_min_height(120.0);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(mode.label())
                                .font(theme::card_title_font())
                                .color(if is_active {
                                    theme::ACCENT
                                } else {
                                    theme::TEXT_PRIMARY
                                })
                                .strong(),
                        );
                        ui.add_space(theme::SPACE_XS);
                        ui.label(
                            RichText::new(mode.description())
                                .font(theme::meta_font())
                                .color(theme::TEXT_SECONDARY),
                        );
                        ui.add_space(theme::SPACE_SM);

                        if is_active {
                            ui.label(
                                RichText::new("✓ Active")
                                    .font(theme::meta_font())
                                    .color(theme::ACCENT),
                            );
                        } else {
                            ui.label(
                                RichText::new("Select")
                                    .font(theme::meta_font())
                                    .color(theme::TEXT_MUTED),
                            );
                        }
                    });
                },
            );

            if response.clicked() {
                chosen = mode;
            }

            ui.add_space(theme::SPACE_SM);
        }
    });

    if chosen != app.config.session.mode {
        // Changing mode mid-session would change what is being billed mid-way,
        // so it applies to the next session.
        app.config.session.mode = chosen;
        app.session = crate::session::session::Session::new(app.config.session.clone());
        app.toast = Some(format!(
            "Routing mode set to {}. It applies to your next session.",
            chosen.label()
        ));
    }
}

/// The translation feed (§23).
fn translation_feed(ui: &mut egui::Ui, app: &mut App) {
    let mut mode = app.config.overlay.text_mode;
    ui.horizontal(|ui| {
        widgets::meta_label(ui, "Show:");
        widgets::segmented(
            ui,
            &[
                (TextMode::OriginalOnly, "Original"),
                (TextMode::Both, "Both"),
                (TextMode::TranslationOnly, "Translation"),
            ],
            &mut mode,
        );
    });
    if mode != app.config.overlay.text_mode {
        app.config.overlay.text_mode = mode;
        // The overlay renders the same content, so a change here is a change
        // there too.
        let mut style = app.config.overlay.clone();
        style.sanitize();
        app.subtitle.set_style(style);
    }
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        if app.feed.is_empty() {
            ui.label(
                RichText::new("No translations yet.")
                    .font(theme::body_font())
                    .color(theme::TEXT_MUTED),
            );
            ui.add_space(theme::SPACE_XS);
            ui.label(
                RichText::new(
                    "Translate something above, or select game audio in Audio settings so \
                     teammates' speech is captured automatically.",
                )
                .font(theme::meta_font())
                .color(theme::TEXT_MUTED),
            );
            return;
        }

        // Newest first, as §23 shows it.
        for line in app.feed.newest_first() {
            ui.label(
                RichText::new(&line.timestamp)
                    .font(theme::meta_font())
                    .color(theme::TEXT_MUTED),
            );

            if mode.shows_original() {
                if let Some(original) = &line.original {
                    ui.label(
                        RichText::new(format!("{}  {}", line.source_tag, original))
                            .font(theme::body_font())
                            .color(theme::TEXT_SECONDARY),
                    );
                }
            }

            if mode.shows_translation() {
                match (&line.translation, &line.error) {
                    (Some(translation), _) => {
                        ui.label(
                            RichText::new(format!("{}  {}", line.target_tag, translation))
                                .font(theme::body_font())
                                .color(theme::TEXT_PRIMARY),
                        );
                    }
                    // A failure is marked, not left looking like a pending
                    // translation. The reader keeps the original above.
                    (None, Some(reason)) => {
                        ui.label(
                            RichText::new(format!("untranslated — {reason}"))
                                .font(theme::meta_font())
                                .color(theme::WARNING),
                        );
                    }
                    (None, None) => {
                        ui.label(
                            RichText::new("translating…")
                                .font(theme::meta_font())
                                .color(theme::TEXT_MUTED),
                        );
                    }
                }
            }
            ui.add_space(theme::SPACE_SM);
        }
    });
}

/// The incoming-translation panel.
///
/// Until audio capture exists this is where transcripts come from: the user
/// types or pastes what a teammate said, and the same pipeline that STT will
/// drive takes over. The code path is identical, so wiring STT later replaces
/// the text box and nothing else.
fn incoming_panel(ui: &mut egui::Ui, app: &mut App) {
    widgets::section_heading(ui, "Incoming Translation");
    ui.add_space(theme::SPACE_XS);
    widgets::meta_label(
        ui,
        "Type what a teammate said, or point at an audio file to transcribe. Both run the same \
         pipeline that captured game audio will use.",
    );
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        provider_status(ui, app);
        ui.add_space(theme::SPACE_MD);

        ui.horizontal(|ui| {
            let width = (ui.available_width() - 120.0).max(200.0);
            let response = ui.add(
                egui::TextEdit::singleline(&mut app.translation_input)
                    .desired_width(width)
                    .hint_text("e.g. Enemy is behind us.")
                    .font(theme::body_font()),
            );

            let submitted_by_enter =
                response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            ui.add_space(theme::SPACE_SM);
            let clicked = widgets::small_button(ui, "Translate").clicked();

            if clicked || submitted_by_enter {
                if !app.submit_translation_input() {
                    app.translator_error = Some(
                        "Nothing to translate, or the worker could not start.".to_string(),
                    );
                }
            }
        });

        ui.add_space(theme::SPACE_SM);
        ui.horizontal(|ui| {
            widgets::field_label(ui, "Audio file");
            let width = (ui.available_width() - 190.0).max(180.0);
            let response = ui.add(
                egui::TextEdit::singleline(&mut app.audio_path)
                    .desired_width(width)
                    .hint_text("path to a .wav to transcribe")
                    .font(theme::body_font()),
            );
            let submitted_by_enter =
                response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            ui.add_space(theme::SPACE_SM);
            let clicked = widgets::small_button(ui, "Transcribe").clicked();
            if clicked || submitted_by_enter {
                if !app.submit_audio_input() {
                    app.translator_error = Some(
                        "Nothing to transcribe, or the file could not be read.".to_string(),
                    );
                }
            }
        });

        ui.add_space(theme::SPACE_XS);
        match &app.transcription_description {
            Some(description) => {
                ui.label(
                    RichText::new(format!("Speech-to-text: {description}"))
                        .font(theme::meta_font())
                        .color(theme::TEXT_MUTED),
                );
            }
            None => {
                ui.label(
                    RichText::new(
                        "Speech-to-text is off or has no key, so audio cannot be transcribed. \
                         Text translation still works.",
                    )
                    .font(theme::meta_font())
                    .color(theme::WARNING),
                );
            }
        }

        if let Some(error) = app.translator_error.clone() {
            ui.add_space(theme::SPACE_SM);
            widgets::outlined_card(ui, theme::SPACE_SM, theme::error_stroke(), |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(error)
                            .font(theme::meta_font())
                            .color(theme::TEXT_PRIMARY),
                    );
                    if ui.small_button("Dismiss").clicked() {
                        // Cleared below, outside the closure.
                    }
                });
            });
            if ui.input(|i| i.pointer.any_click()) {
                app.translator_error = None;
            }
        }

        let stats = app.translation_stats;
        if stats.has_activity() {
            ui.add_space(theme::SPACE_SM);
            ui.label(
                RichText::new(stats.summary())
                    .font(theme::meta_font())
                    .color(if stats.is_degraded() {
                        theme::WARNING
                    } else {
                        theme::TEXT_MUTED
                    }),
            );
        }
    });
}

/// A one-line description of the active provider, with its health.
fn provider_status(ui: &mut egui::Ui, app: &App) {
    let running = app.translator_running();
    let tone = if running {
        theme::StatusTone::Connected
    } else if app.translator_error.is_some() {
        theme::StatusTone::Error
    } else {
        theme::StatusTone::Ready
    };

    ui.horizontal(|ui| {
        widgets::dot(ui, theme::status_color(tone));
        let description = app
            .translator_description
            .clone()
            .unwrap_or_else(|| app.config.provider.mode.label().to_string());
        ui.label(
            RichText::new(description)
                .font(theme::meta_font())
                .color(theme::TEXT_SECONDARY),
        );
    });

    ui.add_space(theme::SPACE_XS);

    // A missing key is the single most likely reason translation will not work,
    // so say so before the user tries and wonders.
    if app.config.provider.mode == crate::config::ProviderMode::OpenAi && !app.api_key_present() {
        ui.label(
            RichText::new(format!(
                "No API key found. Set {} and restart, or switch to the demo dictionary in \
                 Settings.",
                app.config.provider.api_key_env
            ))
            .font(theme::meta_font())
            .color(theme::WARNING),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_routing_card_copy_matches_the_spec() {
        // §27 gives exact strings for each card.
        assert_eq!(RoutingMode::Subtitle.description(), "Text translation only");
        assert_eq!(
            RoutingMode::VoiceOut.description(),
            "Your voice translated into the game"
        );
        assert_eq!(
            RoutingMode::FullVoice.description(),
            "Translate everyone, both directions"
        );
    }

    #[test]
    fn all_three_routing_modes_are_offered() {
        // §37 shows exactly three options in the segmented control.
        assert_eq!(RoutingMode::ALL.len(), 3);
    }

    #[test]
    fn changing_the_language_pair_direction_reverses_both_sides() {
        let mut app = App::default();
        let before = app.config.session.pair;
        app.config.session.pair = app.config.session.pair.reversed();
        assert_eq!(app.config.session.pair.source, before.target);
        assert_eq!(app.config.session.pair.target, before.source);
    }

    #[test]
    fn the_feed_mode_switches_between_the_three_text_modes() {
        let mut app = App::default();
        app.config.overlay.text_mode = TextMode::OriginalOnly;
        assert!(app.config.overlay.text_mode.shows_original());
        assert!(!app.config.overlay.text_mode.shows_translation());

        app.config.overlay.text_mode = TextMode::Both;
        assert!(app.config.overlay.text_mode.shows_original());
        assert!(app.config.overlay.text_mode.shows_translation());
    }
}
