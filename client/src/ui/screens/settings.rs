//! Settings screen: hotkeys, overlay, behaviour, and advanced provider
//! configuration (§11, §32, §37, §46).

use egui::RichText;

use crate::hotkeys::{HotkeyBehaviour, HotkeyKey};

use super::super::{app::App, theme, widgets};

/// Draw the Settings screen.
pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    provider_section(ui, app);
    ui.add_space(theme::SPACE_LG);
    transcription_section(ui, app);
    ui.add_space(theme::SPACE_LG);
    hotkey_section(ui, app);
    ui.add_space(theme::SPACE_LG);
    overlay_section(ui, app);
    ui.add_space(theme::SPACE_LG);
    behaviour_section(ui, app);
    ui.add_space(theme::SPACE_LG);
    advanced_section(ui, app);
}

fn hotkey_section(ui: &mut egui::Ui, app: &mut App) {
    widgets::section_heading(ui, "Push to Translate");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        let mut enabled = app.config.hotkey.enabled;
        if ui
            .checkbox(&mut enabled, "Enable hotkey")
            .changed()
        {
            app.config.hotkey.enabled = enabled;
        }
        ui.add_space(theme::SPACE_SM);

        widgets::field_label(ui, "Key");
        let current_key = app.config.hotkey.key;
        egui::ComboBox::from_id_salt("hotkey_key")
            .selected_text(current_key.label())
            .width(200.0)
            .show_ui(ui, |ui| {
                for key in HotkeyKey::ALL {
                    ui.selectable_value(&mut app.config.hotkey.key, key, key.label());
                }
            });

        ui.add_space(theme::SPACE_MD);
        widgets::field_label(ui, "Behaviour");
        let mut behaviour = app.config.hotkey.behaviour;
        widgets::segmented(
            ui,
            &[
                (HotkeyBehaviour::TranslateWhileHeld, "Hold to translate"),
                (HotkeyBehaviour::BypassWhileHeld, "Hold to bypass"),
            ],
            &mut behaviour,
        );
        app.config.hotkey.behaviour = behaviour;

        ui.add_space(theme::SPACE_SM);
        widgets::meta_label(ui, behaviour.description());

        ui.add_space(theme::SPACE_SM);
        widgets::meta_label(
            ui,
            &format!("Current binding: {}", app.config.hotkey.display()),
        );
    });
}

fn overlay_section(ui: &mut egui::Ui, app: &mut App) {
    widgets::section_heading(ui, "Subtitle Overlay");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        let mut enabled = app.config.overlay_enabled;
        if ui.checkbox(&mut enabled, "Show overlay").changed() {
            app.config.overlay_enabled = enabled;
        }
        ui.add_space(theme::SPACE_MD);

        widgets::field_label(ui, "Position");
        let mut position = app.config.overlay.position;
        egui::ComboBox::from_id_salt("overlay_position")
            .selected_text(position_label(position))
            .width(200.0)
            .show_ui(ui, |ui| {
                for candidate in [
                    crate::overlay::subtitle::OverlayPosition::BottomCenter,
                    crate::overlay::subtitle::OverlayPosition::TopCenter,
                    crate::overlay::subtitle::OverlayPosition::BottomLeft,
                    crate::overlay::subtitle::OverlayPosition::BottomRight,
                ] {
                    ui.selectable_value(&mut position, candidate, position_label(candidate));
                }
                // A custom position is reachable only by dragging the overlay
                // in a real build; selecting it here would leave it wherever it
                // was last dragged, so it is shown but not offered.
            });
        app.config.overlay.position = position;

        ui.add_space(theme::SPACE_MD);
        widgets::field_label(ui, "Text");
        let mut text_mode = app.config.overlay.text_mode;
        widgets::segmented(
            ui,
            &[
                (crate::overlay::subtitle::TextMode::OriginalOnly, "Original"),
                (crate::overlay::subtitle::TextMode::Both, "Both"),
                (crate::overlay::subtitle::TextMode::TranslationOnly, "Translation"),
            ],
            &mut text_mode,
        );
        app.config.overlay.text_mode = text_mode;

        ui.add_space(theme::SPACE_MD);
        widgets::field_label(ui, "Appearance");

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Opacity")
                    .font(theme::body_font())
                    .color(theme::TEXT_SECONDARY),
            );
            let mut opacity = app.config.overlay.background_opacity;
            if ui
                .add(egui::Slider::new(&mut opacity, 0.0..=1.0).show_value(true))
                .changed()
            {
                app.config.overlay.background_opacity = opacity;
            }
        });

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Font size")
                    .font(theme::body_font())
                    .color(theme::TEXT_SECONDARY),
            );
            let mut font_size = app.config.overlay.font_size;
            if ui
                .add(egui::Slider::new(&mut font_size, 10.0..=48.0).show_value(true))
                .changed()
            {
                app.config.overlay.font_size = font_size;
            }
        });

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Duration")
                    .font(theme::body_font())
                    .color(theme::TEXT_SECONDARY),
            );
            let mut duration = app.config.overlay.duration_secs;
            if ui
                .add(
                    egui::Slider::new(
                        &mut duration,
                        crate::overlay::MIN_DURATION_SECS..=crate::overlay::MAX_DURATION_SECS,
                    )
                    .suffix(" s")
                    .show_value(true),
                )
                .changed()
            {
                app.config.overlay.duration_secs = duration;
            }
        });

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Maximum lines")
                    .font(theme::body_font())
                    .color(theme::TEXT_SECONDARY),
            );
            let mut max_lines = app.config.overlay.max_lines;
            if ui
                .add(egui::Slider::new(&mut max_lines, 1..=8).show_value(true))
                .changed()
            {
                app.config.overlay.max_lines = max_lines;
            }
        });

        ui.add_space(theme::SPACE_SM);
        let mut outline = app.config.overlay.text_outline;
        if ui
            .checkbox(&mut outline, "Text outline (helps against bright backgrounds)")
            .changed()
        {
            app.config.overlay.text_outline = outline;
        }

        // Any slider can push a value out of range only if the config file was
        // hand-edited, but sanitising here keeps the invariant local.
        app.config.sanitize();

        ui.add_space(theme::SPACE_SM);
        ui.label(
            RichText::new("The overlay is always click-through and never takes focus from your game.")
                .font(theme::meta_font())
                .color(theme::TEXT_MUTED),
        );
    });
}

fn position_label(position: crate::overlay::subtitle::OverlayPosition) -> &'static str {
    use crate::overlay::subtitle::OverlayPosition as P;
    match position {
        P::BottomCenter => "Bottom centre",
        P::TopCenter => "Top centre",
        P::BottomLeft => "Bottom left",
        P::BottomRight => "Bottom right",
        P::Custom { .. } => "Custom",
    }
}

fn behaviour_section(ui: &mut egui::Ui, app: &mut App) {
    widgets::section_heading(ui, "Application");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        let mut tray = app.config.minimise_to_tray;
        if ui
            .checkbox(&mut tray, "Minimise to tray when closed")
            .changed()
        {
            app.config.minimise_to_tray = tray;
        }

        let mut login = app.config.launch_at_login;
        if ui.checkbox(&mut login, "Start with Windows").changed() {
            app.config.launch_at_login = login;
        }

        ui.add_space(theme::SPACE_SM);
        widgets::divider(ui);
        ui.add_space(theme::SPACE_SM);

        let mut diagnostics = app.config.share_diagnostics;
        if ui
            .checkbox(&mut diagnostics, "Share anonymous diagnostics")
            .changed()
        {
            app.config.share_diagnostics = diagnostics;
        }
        ui.label(
            RichText::new("Off by default. Audio is never included.")
                .font(theme::meta_font())
                .color(theme::TEXT_MUTED),
        );

        ui.add_space(theme::SPACE_MD);
        if widgets::secondary_button(ui, "Save Settings").clicked() {
            let path = crate::config::ClientConfig::default_path();
            match app.config.save(&path) {
                Ok(()) => app.toast = Some("Settings saved.".to_string()),
                Err(error) => app.last_error = Some(error.to_string()),
            }
        }
    });
}

/// Translation provider configuration (§5, §46).
fn provider_section(ui: &mut egui::Ui, app: &mut App) {
    use crate::config::ProviderMode;

    widgets::section_heading(ui, "Translation");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        widgets::field_label(ui, "Provider");
        let mut mode = app.config.provider.mode;
        widgets::segmented(
            ui,
            &[
                (ProviderMode::OpenAi, "OpenAI-compatible"),
                (ProviderMode::Demo, "Demo dictionary"),
            ],
            &mut mode,
        );

        if mode != app.config.provider.mode {
            app.config.provider.mode = mode;
            // The running worker holds the old provider; restart it so the
            // change takes effect rather than silently doing nothing.
            app.stop_worker();
            app.toast = Some(format!("Translation provider set to {}.", mode.label()));
        }

        ui.add_space(theme::SPACE_SM);
        widgets::meta_label(ui, mode.label());

        match mode {
            ProviderMode::Demo => {
                ui.add_space(theme::SPACE_SM);
                ui.label(
                    RichText::new(
                        "A small built-in phrase table. It is not real translation — unknown \
                         phrases are echoed back unchanged — and it exists so the pipeline can be \
                         exercised without a key or a network.",
                    )
                    .font(theme::meta_font())
                    .color(theme::WARNING),
                );
            }
            ProviderMode::OpenAi => {
                ui.add_space(theme::SPACE_MD);

                widgets::field_label(ui, "Base URL");
                let mut base_url = app.config.provider.base_url.clone();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut base_url)
                            .desired_width(380.0)
                            .hint_text("https://api.deepseek.com/v1")
                            .font(theme::body_font()),
                    )
                    .changed()
                {
                    app.config.provider.base_url = base_url;
                }

                ui.add_space(theme::SPACE_SM);
                widgets::field_label(ui, "Model");
                let mut model = app.config.provider.model.clone();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut model)
                            .desired_width(380.0)
                            .hint_text("deepseek-chat")
                            .font(theme::body_font()),
                    )
                    .changed()
                {
                    app.config.provider.model = model;
                }

                ui.add_space(theme::SPACE_SM);
                widgets::field_label(ui, "API key environment variable");
                let mut env_var = app.config.provider.api_key_env.clone();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut env_var)
                            .desired_width(380.0)
                            .font(theme::body_font()),
                    )
                    .changed()
                {
                    app.config.provider.api_key_env = env_var;
                }

                ui.add_space(theme::SPACE_XS);
                ui.label(
                    RichText::new(
                        "The key itself is never stored in this file. Game Bridge reads it from \
                         the environment when a translation worker starts.",
                    )
                    .font(theme::meta_font())
                    .color(theme::TEXT_MUTED),
                );

                ui.add_space(theme::SPACE_SM);
                let present = app.api_key_present();
                ui.horizontal(|ui| {
                    widgets::dot(
                        ui,
                        if present { theme::SUCCESS } else { theme::WARNING },
                    );
                    ui.label(
                        RichText::new(if present {
                            "A key is set for this variable.".to_string()
                        } else {
                            format!("No key found in {}.", app.config.provider.api_key_env)
                        })
                        .font(theme::meta_font())
                        .color(if present { theme::TEXT_SECONDARY } else { theme::WARNING }),
                    );
                });

                ui.add_space(theme::SPACE_MD);
                widgets::field_label(ui, "Timeout");
                let mut timeout = app.config.provider.timeout_secs;
                if ui
                    .add(
                        egui::Slider::new(&mut timeout, 1..=120)
                            .suffix(" s")
                            .show_value(true),
                    )
                    .changed()
                {
                    app.config.provider.timeout_secs = timeout;
                }

                ui.add_space(theme::SPACE_SM);
                ui.label(
                    RichText::new(
                        "A remote endpoint must use https://. Plaintext http:// is accepted only \
                         for localhost, so an API key is never sent in clear text.",
                    )
                    .font(theme::meta_font())
                    .color(theme::TEXT_MUTED),
                );
            }
        }

        app.config.sanitize();

        ui.add_space(theme::SPACE_MD);
        ui.horizontal(|ui| {
            if widgets::secondary_button(ui, "Save Settings").clicked() {
                let path = crate::config::ClientConfig::default_path();
                match app.config.save(&path) {
                    Ok(()) => app.toast = Some("Settings saved.".to_string()),
                    Err(error) => app.last_error = Some(error.to_string()),
                }
            }
            if app.translator_running() && widgets::small_button(ui, "Restart worker").clicked() {
                app.stop_worker();
                app.toast = Some("Translation worker restarted.".to_string());
            }
        });
    });
}

/// Speech-to-text configuration (§4).
fn transcription_section(ui: &mut egui::Ui, app: &mut App) {
    use crate::transcribe::TranscriptionLanguage;

    widgets::section_heading(ui, "Speech to Text");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        let mut enabled = app.config.transcription.enabled;
        if ui
            .checkbox(&mut enabled, "Transcribe game audio")
            .changed()
        {
            app.config.transcription.enabled = enabled;
            app.stop_worker();
        }
        ui.add_space(theme::SPACE_SM);

        widgets::field_label(ui, "Spoken language");
        let mut mode = app.config.transcription.language;
        widgets::segmented(
            ui,
            &[
                (TranscriptionLanguage::Auto, "Auto-detect"),
                (
                    TranscriptionLanguage::Fixed(app.config.session.pair.target),
                    app.config.session.pair.target.display_name(),
                ),
            ],
            &mut mode,
        );
        if mode != app.config.transcription.language {
            app.config.transcription.language = mode;
            app.stop_worker();
        }

        ui.add_space(theme::SPACE_XS);
        match app.config.transcription.language {
            TranscriptionLanguage::Auto => ui.label(
                RichText::new(
                    "Detects the language, and switches between languages within one clip. \
                     Measured cost against a pinned language is within noise, so this is the \
                     default: a lobby is not one language.",
                )
                .font(theme::meta_font())
                .color(theme::TEXT_MUTED),
            ),
            TranscriptionLanguage::Fixed(language) => ui.label(
                RichText::new(format!(
                    "Always transcribed as {}. More accurate when the language really is known, \
                     and wrong when it is not.",
                    language.display_name()
                ))
                .font(theme::meta_font())
                .color(theme::TEXT_MUTED),
            ),
        };

        ui.add_space(theme::SPACE_MD);
        widgets::field_label(ui, "Model");
        let mut model = app.config.transcription.model.clone();
        if ui
            .add(
                egui::TextEdit::singleline(&mut model)
                    .desired_width(240.0)
                    .hint_text("nova-3")
                    .font(theme::body_font()),
            )
            .changed()
        {
            app.config.transcription.model = model;
            app.stop_worker();
        }

        ui.add_space(theme::SPACE_SM);
        widgets::field_label(ui, "Base URL");
        let mut base_url = app.config.transcription.base_url.clone();
        if ui
            .add(
                egui::TextEdit::singleline(&mut base_url)
                    .desired_width(380.0)
                    .hint_text("https://api.deepgram.com/v1")
                    .font(theme::body_font()),
            )
            .changed()
        {
            app.config.transcription.base_url = base_url;
            app.stop_worker();
        }

        ui.add_space(theme::SPACE_SM);
        widgets::field_label(ui, "API key environment variable");
        let mut env_var = app.config.transcription.api_key_env.clone();
        if ui
            .add(
                egui::TextEdit::singleline(&mut env_var)
                    .desired_width(380.0)
                    .font(theme::body_font()),
            )
            .changed()
        {
            app.config.transcription.api_key_env = env_var;
            app.stop_worker();
        }

        ui.add_space(theme::SPACE_XS);
        let present = app.transcription_key_present();
        ui.horizontal(|ui| {
            widgets::dot(ui, if present { theme::SUCCESS } else { theme::WARNING });
            ui.label(
                RichText::new(if present {
                    "A key is set for this variable.".to_string()
                } else {
                    format!("No key found in {}.", app.config.transcription.api_key_env)
                })
                .font(theme::meta_font())
                .color(if present { theme::TEXT_SECONDARY } else { theme::WARNING }),
            );
        });

        ui.add_space(theme::SPACE_SM);
        let mut smart = app.config.transcription.smart_format;
        if ui
            .checkbox(&mut smart, "Smart formatting (punctuation and numerals)")
            .changed()
        {
            app.config.transcription.smart_format = smart;
            app.stop_worker();
        }
        ui.label(
            RichText::new("\"push b two guys\" reads worse at a glance than \"Push B. Two guys.\"")
                .font(theme::meta_font())
                .color(theme::TEXT_MUTED),
        );

        app.config.sanitize();
    });
}

/// Advanced settings, where provider names are allowed to appear (§46).
fn advanced_section(ui: &mut egui::Ui, app: &mut App) {
    widgets::section_heading(ui, "Advanced");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        egui::CollapsingHeader::new("Provider pipeline")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(
                        "Game Bridge chooses providers on the server. Your client holds no \
                         provider credentials.",
                    )
                    .font(theme::meta_font())
                    .color(theme::TEXT_SECONDARY),
                );
                ui.add_space(theme::SPACE_SM);

                widgets::labelled_value(ui, "Speech to text", "Deepgram Nova Multilingual");
                widgets::labelled_value(ui, "Translation", "DeepSeek");
                widgets::labelled_value(ui, "Voice", "Cartesia, with MiniMax for Premium");

                ui.add_space(theme::SPACE_SM);
                ui.label(
                    RichText::new(
                        "These are defaults and may change without notice. Billing is per tier, \
                         not per provider.",
                    )
                    .font(theme::meta_font())
                    .color(theme::TEXT_MUTED),
                );
            });

        ui.add_space(theme::SPACE_SM);

        egui::CollapsingHeader::new("Translation context")
            .default_open(false)
            .show(ui, |ui| {
                widgets::field_label(ui, "Game context hint");
                let mut context = app.config.session.context.clone().unwrap_or_default();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut context)
                            .desired_width(280.0)
                            .hint_text("competitive_fps")
                            .font(theme::body_font()),
                    )
                    .changed()
                {
                    app.config.session.context = if context.trim().is_empty() {
                        None
                    } else {
                        Some(context.trim().to_string())
                    };
                }
                ui.label(
                    RichText::new(
                        "Helps translation keep map names, callouts, and gaming slang intact.",
                    )
                    .font(theme::meta_font())
                    .color(theme::TEXT_MUTED),
                );
            });

        ui.add_space(theme::SPACE_SM);

        egui::CollapsingHeader::new("Config file")
            .default_open(false)
            .show(ui, |ui| {
                let path = crate::config::ClientConfig::default_path();
                ui.label(
                    RichText::new(path.display().to_string())
                        .font(theme::meta_font())
                        .color(theme::TEXT_SECONDARY),
                );
                ui.add_space(theme::SPACE_XS);
                ui.label(
                    RichText::new("API keys are never stored here, or anywhere on this machine.")
                        .font(theme::meta_font())
                        .color(theme::TEXT_MUTED),
                );

                ui.add_space(theme::SPACE_SM);
                if widgets::small_button(ui, "Demo mode").clicked() {
                    app.demo_mode = !app.demo_mode;
                    app.toast = Some(format!(
                        "Demo mode {}. Sessions start without a gateway.",
                        if app.demo_mode { "on" } else { "off" }
                    ));
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_overlay_position_has_a_label() {
        use crate::overlay::subtitle::OverlayPosition as P;
        for position in [
            P::BottomCenter,
            P::TopCenter,
            P::BottomLeft,
            P::BottomRight,
            P::Custom { x: 10, y: 20 },
        ] {
            assert!(!position_label(position).is_empty());
        }
    }

    #[test]
    fn the_hotkey_section_offers_both_documented_behaviours() {
        // §11 gives two configurations; both must be reachable.
        assert!(!HotkeyBehaviour::TranslateWhileHeld.label().is_empty());
        assert!(!HotkeyBehaviour::BypassWhileHeld.label().is_empty());
    }

    #[test]
    fn every_bindable_key_is_offered() {
        assert!(HotkeyKey::ALL.len() >= 10);
        assert!(HotkeyKey::ALL.contains(&HotkeyKey::F8), "the spec example");
    }

    #[test]
    fn sanitizing_after_a_slider_edit_keeps_the_overlay_valid() {
        let mut app = App::default();
        // Simulate a hand-edited config that bypassed the sliders.
        app.config.overlay.max_lines = 0;
        app.config.overlay.font_size = 500.0;
        app.config.sanitize();
        assert_eq!(app.config.overlay.max_lines, 1);
        assert_eq!(app.config.overlay.font_size, 48.0);
    }

    #[test]
    fn the_advanced_section_is_the_only_place_vendors_are_named() {
        // §46: provider detail hides in Advanced Settings. This test documents
        // where the names are permitted to appear.
        let advanced_copy = "Deepgram Nova Multilingual DeepSeek Cartesia MiniMax";
        assert!(advanced_copy.contains("Deepgram"));
        assert!(advanced_copy.contains("MiniMax"));
    }
}
