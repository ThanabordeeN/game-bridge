//! Voice screen: the user's voice, provider tiers, and clone enrollment
//! (§24, §25).

use egui::RichText;

use game_bridge_protocol::mode::VoiceTier;

use super::super::{app::App, theme, widgets};

/// Draw the Voice screen.
pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    widgets::section_heading(ui, "Your Voice");
    ui.add_space(theme::SPACE_SM);
    voice_card(ui, app);
    ui.add_space(theme::SPACE_LG);

    widgets::section_heading(ui, "Voice Provider");
    ui.add_space(theme::SPACE_XS);
    widgets::meta_label(
        ui,
        "The default UI names these by quality tier, not by vendor. Advanced Settings shows \
         which provider serves each.",
    );
    ui.add_space(theme::SPACE_SM);
    tier_selection(ui, app);
    ui.add_space(theme::SPACE_LG);

    if app.config.session.voice_tier == VoiceTier::Cloned {
        clone_setup(ui, app);
    }
}

/// The voice card (§24).
fn voice_card(ui: &mut egui::Ui, app: &mut App) {
    widgets::raised_card(ui, theme::SPACE_XL, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(theme::SPACE_MD);

            // Avatar placeholder: a circle with the tier initial. A real build
            // would render a waveform preview of the enrolled voice.
            let (rect, _) = ui.allocate_exact_size(
                egui::Vec2::splat(96.0),
                egui::Sense::hover(),
            );
            ui.painter()
                .circle_filled(rect.center(), 48.0, theme::SURFACE_HOVER);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                match app.config.session.voice_tier {
                    VoiceTier::Standard => "S",
                    VoiceTier::Cloned => "M",
                    VoiceTier::Premium => "P",
                },
                egui::FontId::proportional(36.0),
                theme::TEXT_PRIMARY,
            );

            ui.add_space(theme::SPACE_MD);
            ui.label(
                RichText::new(match app.config.session.voice_tier {
                    VoiceTier::Standard => "Standard Voice",
                    VoiceTier::Cloned => "My Voice",
                    VoiceTier::Premium => "Premium Voice",
                })
                .font(theme::section_font())
                .color(theme::TEXT_PRIMARY)
                .strong(),
            );
            ui.add_space(theme::SPACE_XS);

            let provider_note = match app.config.session.voice_tier {
                VoiceTier::Standard => "Bundled voice, included with Voice mode",
                VoiceTier::Cloned => "Your cloned voice, generated cross-language",
                VoiceTier::Premium => "Higher-fidelity voice, higher burn rate",
            };
            widgets::meta_label(ui, provider_note);

            ui.add_space(theme::SPACE_MD);
            if widgets::small_button(ui, "▶ Preview").clicked() {
                // Previewing synthesises a short sample. In this scaffold there
                // is no TTS path, so the affordance reports rather than fakes.
                app.toast = Some(
                    "Preview needs a live session and the Gateway. Start a bridge to hear your \
                     voice."
                        .to_string(),
                );
            }
            ui.add_space(theme::SPACE_MD);
        });
    });
}

/// The three-tier selection (§24).
fn tier_selection(ui: &mut egui::Ui, app: &mut App) {
    widgets::card(ui, theme::SPACE_MD, |ui| {
        for tier in [VoiceTier::Standard, VoiceTier::Cloned, VoiceTier::Premium] {
            let selected = app.config.session.voice_tier == tier;
            let mut choice = selected;
            ui.horizontal(|ui| {
                ui.radio_value(&mut choice, true, "");
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(tier.label())
                            .font(theme::body_font())
                            .color(if selected {
                                theme::ACCENT
                            } else {
                                theme::TEXT_PRIMARY
                            }),
                    );
                    ui.label(
                        RichText::new(tier_note(tier))
                            .font(theme::meta_font())
                            .color(theme::TEXT_MUTED),
                    );
                });
            });
            if choice && !selected {
                app.config.session.voice_tier = tier;
            }
            ui.add_space(theme::SPACE_SM);
        }
    });
}

fn tier_note(tier: VoiceTier) -> &'static str {
    match tier {
        VoiceTier::Standard => "Included with Voice mode",
        VoiceTier::Cloned => "Requires a one-time voice recording",
        VoiceTier::Premium => "Higher burn rate per active minute",
    }
}

/// Voice clone enrollment (§25).
fn clone_setup(ui: &mut egui::Ui, app: &mut App) {
    widgets::section_heading(ui, "Create your voice");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        ui.label(
            RichText::new("Speak naturally for a few seconds.")
                .font(theme::body_font())
                .color(theme::TEXT_SECONDARY),
        );
        ui.add_space(theme::SPACE_SM);

        // A progress bar with no live recording source behind it in this
        // scaffold. The flow is documented so the real implementation has a
        // shape to fill, and the button says what it can actually do rather
        // than pretending to record.
        let recording_secs = 8u32;
        let target_secs = 15u32;
        let fraction = recording_secs as f32 / target_secs as f32;

        let (rect, _) = ui.allocate_exact_size(
            egui::Vec2::new(ui.available_width().min(420.0), 8.0),
            egui::Sense::hover(),
        );
        ui.painter().rect_filled(rect, 4.0, theme::SURFACE_HOVER);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                rect.min,
                egui::Vec2::new(rect.width() * fraction, rect.height()),
            ),
            4.0,
            theme::ACCENT,
        );

        ui.add_space(theme::SPACE_SM);
        ui.label(
            RichText::new(format!(
                "{:02}:{:02} / {:02}:{:02}",
                recording_secs / 60,
                recording_secs % 60,
                target_secs / 60,
                target_secs % 60
            ))
            .font(theme::meta_font())
            .color(theme::TEXT_PRIMARY),
        );

        ui.add_space(theme::SPACE_MD);
        if widgets::secondary_button(ui, "Start Recording").clicked() {
            app.toast = Some(
                "Voice cloning needs the Gateway and a signed-in account. This build records \
                 nothing."
                    .to_string(),
            );
        }

        ui.add_space(theme::SPACE_SM);
        ui.label(
            RichText::new(
                "Your recording is sent to the Gateway for enrollment and is not stored on this \
                 device.",
            )
            .font(theme::meta_font())
            .color(theme::TEXT_MUTED),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_three_tiers_are_offered_and_match_the_spec_labels() {
        // §24 names exactly these three, and §16 says the UI shows only these.
        assert_eq!(VoiceTier::Standard.label(), "Standard Voice");
        assert_eq!(VoiceTier::Cloned.label(), "My Cloned Voice");
        assert_eq!(VoiceTier::Premium.label(), "Premium Voice");
    }

    #[test]
    fn only_the_cloned_tier_requires_enrollment() {
        assert!(VoiceTier::Cloned.requires_clone_enrollment());
        assert!(!VoiceTier::Standard.requires_clone_enrollment());
        assert!(!VoiceTier::Premium.requires_clone_enrollment());
    }

    #[test]
    fn every_tier_has_a_note_for_the_list() {
        for tier in [VoiceTier::Standard, VoiceTier::Cloned, VoiceTier::Premium] {
            assert!(!tier_note(tier).is_empty(), "{tier:?}");
        }
    }

    #[test]
    fn the_default_tier_is_standard() {
        assert_eq!(VoiceTier::default(), VoiceTier::Standard);
    }

    #[test]
    fn the_screen_never_names_a_vendor_in_the_default_copy() {
        // §16: "UI ไม่จำเป็นต้องแสดง provider" — the default UI must not leak
        // vendor names. Advanced Settings is the only place they belong.
        for tier in [VoiceTier::Standard, VoiceTier::Cloned, VoiceTier::Premium] {
            let text = format!("{} {}", tier.label(), tier_note(tier));
            let lowered = text.to_lowercase();
            for vendor in ["cartesia", "minimax", "deepgram", "deepseek", "elevenlabs"] {
                assert!(
                    !lowered.contains(vendor),
                    "{tier:?} copy leaks vendor name {vendor:?}"
                );
            }
        }
    }
}
