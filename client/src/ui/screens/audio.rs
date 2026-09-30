//! Audio screen: device selection and the virtual microphone state
//! (§26, §38, §39).

use egui::RichText;

use crate::device::detection::{DeviceRole, VIRTUAL_DEVICE_NAME};

use super::super::{app::App, theme, widgets};

/// Draw the Audio screen.
pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    microphone_section(ui, app);
    ui.add_space(theme::SPACE_LG);
    game_audio_section(ui, app);
    ui.add_space(theme::SPACE_LG);
    monitoring_section(ui, app);
    ui.add_space(theme::SPACE_LG);
    mixer_section(ui, app);
    ui.add_space(theme::SPACE_LG);
    audio_test(ui, app);
}

/// Device selection. Until the WASAPI enumeration lands, these are text fields
/// holding the stable endpoint ID a real build would fill from discovery.
fn device_field(ui: &mut egui::Ui, role: DeviceRole, slot: &mut Option<String>) {
    widgets::field_label(ui, role.label());
    let mut text = slot.clone().unwrap_or_default();
    let response = ui.add(
        egui::TextEdit::singleline(&mut text)
            .desired_width(360.0)
            .hint_text("Select a device…")
            .font(theme::body_font()),
    );
    if response.changed() {
        *slot = if text.trim().is_empty() {
            None
        } else {
            Some(text.trim().to_string())
        };
    }
}

fn microphone_section(ui: &mut egui::Ui, app: &mut App) {
    widgets::section_heading(ui, "Microphone");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        widgets::field_label(ui, DeviceRole::Microphone.label());
        let mut text = app.config.session.microphone_id.clone();
        if ui
            .add(
                egui::TextEdit::singleline(&mut text)
                    .desired_width(360.0)
                    .hint_text("Your physical microphone")
                    .font(theme::body_font()),
            )
            .changed()
        {
            app.config.session.microphone_id = text;
        }

        ui.add_space(theme::SPACE_MD);
        widgets::divider(ui);
        ui.add_space(theme::SPACE_MD);

        // The virtual microphone is the output for translated voice. It is
        // never selectable as an input, which is §13's requirement made visible.
        widgets::field_label(ui, DeviceRole::VirtualMicrophone.label());
        let installed = app.config.session.virtual_mic_id.is_some();
        ui.horizontal(|ui| {
            widgets::readonly_field(
                ui,
                if installed {
                    VIRTUAL_DEVICE_NAME
                } else {
                    "Not installed"
                },
                360.0,
            );
            ui.add_space(theme::SPACE_SM);
            widgets::dot(
                ui,
                if installed {
                    theme::SUCCESS
                } else {
                    theme::ERROR
                },
            );
        });

        ui.add_space(theme::SPACE_SM);
        if installed {
            ui.label(
                RichText::new(
                    "This device carries your translated voice into the game. Select it as your \
                     microphone in Discord or in-game.",
                )
                .font(theme::meta_font())
                .color(theme::TEXT_MUTED),
            );
        } else {
            // §40's exact error state.
            widgets::outlined_card(ui, theme::SPACE_SM, theme::error_stroke(), |ui| {
                ui.label(
                    RichText::new("Virtual Microphone Not Found")
                        .font(theme::card_title_font())
                        .color(theme::ERROR)
                        .strong(),
                );
                ui.add_space(theme::SPACE_XS);
                ui.label(
                    RichText::new("Game Bridge Audio Driver is not installed.")
                        .font(theme::body_font())
                        .color(theme::TEXT_SECONDARY),
                );
                ui.add_space(theme::SPACE_SM);
                if widgets::small_button(ui, "Install Driver").clicked() {
                    app.toast = Some(
                        "Driver installation ships with the signed Game Bridge audio driver. See \
                         virtual-audio-driver/README.md."
                            .to_string(),
                    );
                }
            });
        }

        ui.add_space(theme::SPACE_MD);
        ui.label(
            RichText::new(
                "The virtual microphone is never used as a translation input. Listening to it \
                 would feed translated speech back into translation.",
            )
            .font(theme::meta_font())
            .color(theme::TEXT_MUTED),
        );
    });
}

fn game_audio_section(ui: &mut egui::Ui, app: &mut App) {
    widgets::section_heading(ui, "Game Audio");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        widgets::meta_label(
            ui,
            "Capturing a game's audio lets Game Bridge subtitle what your teammates say.",
        );
        ui.add_space(theme::SPACE_SM);

        widgets::field_label(ui, "Application");
        let current = app
            .config
            .session
            .application
            .clone()
            .unwrap_or_else(|| "None".to_string());

        egui::ComboBox::from_id_salt("game_audio_app")
            .selected_text(current)
            .width(360.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(false, "None").clicked() {
                    app.config.session.application = None;
                }
                // Populated from process loopback discovery in a real build;
                // the options below are the ones §19 and §28 name.
                for candidate in ["Valorant.exe", "Delta Force.exe", "Discord.exe"] {
                    if ui
                        .selectable_label(
                            app.config.session.application.as_deref() == Some(candidate),
                            candidate,
                        )
                        .clicked()
                    {
                        app.config.session.application = Some(candidate.to_string());
                    }
                }
            });

        ui.add_space(theme::SPACE_SM);
        ui.label(
            RichText::new("Requires Windows 10 version 2004 or newer for per-application capture.")
                .font(theme::meta_font())
                .color(theme::TEXT_MUTED),
        );
    });
}

fn monitoring_section(ui: &mut egui::Ui, app: &mut App) {
    widgets::section_heading(ui, "Monitoring");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        let mut headphones = app.config.session.headphones_id.clone();
        device_field(ui, DeviceRole::Headphones, &mut headphones);
        app.config.session.headphones_id = headphones;

        ui.add_space(theme::SPACE_SM);
        ui.label(
            RichText::new(
                "Needed only in Full Voice mode, where other players' speech is voiced into your \
                 headphones.",
            )
            .font(theme::meta_font())
            .color(theme::TEXT_MUTED),
        );
    });
}

/// Mixer balance (§12).
fn mixer_section(ui: &mut egui::Ui, app: &mut App) {
    widgets::section_heading(ui, "Audio Mixing");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        let mut original = app.config.mix.original.percent();
        let mut translated = app.config.mix.translated.percent();

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Original Voice")
                    .font(theme::body_font())
                    .color(theme::TEXT_PRIMARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{original}%"))
                        .font(theme::meta_font())
                        .color(theme::TEXT_SECONDARY),
                );
            });
        });
        ui.add(egui::Slider::new(&mut original, 0..=100).show_value(false));

        ui.add_space(theme::SPACE_SM);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Translated Voice")
                    .font(theme::body_font())
                    .color(theme::TEXT_PRIMARY),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{translated}%"))
                        .font(theme::meta_font())
                        .color(theme::TEXT_SECONDARY),
                );
            });
        });
        ui.add(egui::Slider::new(&mut translated, 0..=100).show_value(false));

        app.config.mix = crate::audio::mixer::MixSettings {
            original: crate::audio::mixer::Gain::new(original),
            translated: crate::audio::mixer::Gain::new(translated),
        };

        ui.add_space(theme::SPACE_MD);
        widgets::field_label(ui, "Presets");
        ui.horizontal(|ui| {
            if widgets::small_button(ui, "Translation only").clicked() {
                app.config.mix = crate::audio::mixer::MixSettings::translation_only();
            }
            if widgets::small_button(ui, "Mixed").clicked() {
                app.config.mix = crate::audio::mixer::MixSettings::mixed();
            }
            if widgets::small_button(ui, "Bypass").clicked() {
                app.config.mix = crate::audio::mixer::MixSettings::bypass();
            }
        });
    });
}

/// The three-step audio test (§39).
fn audio_test(ui: &mut egui::Ui, app: &mut App) {
    widgets::section_heading(ui, "Test Audio");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        ui.label(
            RichText::new("Validate the whole path before you get into a match.")
                .font(theme::body_font())
                .color(theme::TEXT_SECONDARY),
        );
        ui.add_space(theme::SPACE_MD);

        for (index, (title, detail)) in [
            ("Microphone", "Speak and check that the meter moves."),
            ("Translation", "Your words come back in the other language."),
            ("Voice", "The translated voice reaches the virtual microphone."),
        ]
        .iter()
        .enumerate()
        {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{}.", index + 1))
                        .font(theme::meta_font())
                        .color(theme::TEXT_MUTED),
                );
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(*title)
                            .font(theme::body_font())
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.label(
                        RichText::new(*detail)
                            .font(theme::meta_font())
                            .color(theme::TEXT_MUTED),
                    );
                });
            });
            ui.add_space(theme::SPACE_SM);
        }

        // The test needs a live session, so the affordance says so rather than
        // presenting a button that silently does nothing.
        let ready = app.config.session.microphone_id.trim().is_empty() == false;
        ui.add_enabled_ui(ready, |ui| {
            if widgets::secondary_button(ui, "Run Test").clicked() {
                app.toast = Some(
                    "The audio test runs during a live session. Start a bridge and speak."
                        .to_string(),
                );
            }
        });
        if !ready {
            ui.add_space(theme::SPACE_XS);
            ui.label(
                RichText::new("Select a microphone first.")
                    .font(theme::meta_font())
                    .color(theme::WARNING),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::mixer::MixSettings;

    #[test]
    fn device_roles_have_the_section_labels_from_the_spec() {
        // §26 headings and field labels.
        assert_eq!(DeviceRole::Microphone.label(), "Microphone");
        assert_eq!(DeviceRole::VirtualMicrophone.label(), "Virtual Output");
        assert_eq!(DeviceRole::ApplicationAudio.section(), "GAME AUDIO");
        assert_eq!(DeviceRole::Headphones.section(), "MONITORING");
    }

    #[test]
    fn the_virtual_mic_label_is_the_name_users_select_in_discord() {
        assert_eq!(VIRTUAL_DEVICE_NAME, "Game Bridge Microphone");
    }

    #[test]
    fn mixer_presets_set_the_documented_percentages() {
        let mut app = App::default();
        app.config.mix = MixSettings::translation_only();
        assert_eq!(app.config.mix.original.percent(), 0);
        assert_eq!(app.config.mix.translated.percent(), 100);

        app.config.mix = MixSettings::bypass();
        assert_eq!(app.config.mix.original.percent(), 100);
        assert_eq!(app.config.mix.translated.percent(), 0);
    }

    #[test]
    fn an_unset_microphone_blocks_the_audio_test() {
        let app = App::default();
        assert!(app.config.session.microphone_id.trim().is_empty());
    }

    #[test]
    fn the_driver_install_flow_is_gated_on_the_driver_being_absent() {
        let mut app = App::default();
        assert!(!app.virtual_mic_ready(), "no driver configured yet");
        app.config.session.virtual_mic_id = Some("{virtual}".into());
        assert!(app.virtual_mic_ready());
    }
}
