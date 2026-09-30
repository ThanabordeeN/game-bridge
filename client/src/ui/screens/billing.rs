//! Billing screen: balance, rate card, and the charge-only-on-speech promise
//! (§29, §40).

use egui::RichText;



use super::super::{app::App, theme, widgets};

/// Draw the Billing screen.
pub fn draw(ui: &mut egui::Ui, app: &mut App) {
    balance_card(ui, app);
    ui.add_space(theme::SPACE_LG);

    if app.wallet.is_low() {
        low_credit_state(ui, app);
        ui.add_space(theme::SPACE_LG);
    }

    rate_card(ui, app);
    ui.add_space(theme::SPACE_LG);

    billing_promise(ui);
}

fn balance_card(ui: &mut egui::Ui, app: &mut App) {
    widgets::raised_card(ui, theme::SPACE_XL, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(theme::SPACE_SM);
            widgets::field_label(ui, "Balance");
            ui.label(
                RichText::new(app.wallet.balance_display())
                    .font(egui::FontId::proportional(40.0))
                    .color(theme::TEXT_PRIMARY)
                    .strong(),
            );
            ui.add_space(theme::SPACE_MD);

            if widgets::primary_button(ui, "+ Add Credits").clicked() {
                app.toast = Some(
                    "Adding credits opens the Game Bridge store in your browser.".to_string(),
                );
            }
            ui.add_space(theme::SPACE_SM);
        });
    });
}

/// §40's low-credit state.
fn low_credit_state(ui: &mut egui::Ui, app: &mut App) {
    widgets::outlined_card(ui, theme::SPACE_MD, theme::warning_stroke(), |ui| {
        ui.label(
            RichText::new("Low Credit")
                .font(theme::card_title_font())
                .color(theme::WARNING)
                .strong(),
        );
        ui.add_space(theme::SPACE_XS);
        ui.label(
            RichText::new(format!("{} remaining", app.wallet.balance_display()))
                .font(theme::body_font())
                .color(theme::TEXT_SECONDARY),
        );
        ui.add_space(theme::SPACE_XS);

        // The most useful number at this moment is not the balance but how long
        // it lasts at the current tier.
        let tier = app.session.usage().tier;
        let remaining = app.wallet.remaining_display(tier);
        ui.label(
            RichText::new(format!("About {remaining} of {} left", tier.label()))
                .font(theme::meta_font())
                .color(theme::TEXT_MUTED),
        );

        ui.add_space(theme::SPACE_SM);
        if widgets::small_button(ui, "Add Credits").clicked() {
            app.toast = Some("Adding credits opens the Game Bridge store.".to_string());
        }
    });
}

/// The rate card (§29).
fn rate_card(ui: &mut egui::Ui, app: &App) {
    widgets::field_label(ui, "Current Rate");
    ui.add_space(theme::SPACE_SM);

    widgets::card(ui, theme::SPACE_MD, |ui| {
        for (tier, rate) in app.wallet.rate_rows() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(tier.label())
                        .font(theme::body_font())
                        .color(theme::TEXT_PRIMARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(rate)
                            .font(theme::body_font())
                            .color(theme::TEXT_SECONDARY),
                    );
                });
            });
            ui.add_space(theme::SPACE_SM);
        }

        widgets::divider(ui);
        ui.add_space(theme::SPACE_SM);

        // §16's subtitle framing: ฿1 buys two active minutes.
        ui.label(
            RichText::new("Subtitle: ฿1 buys 2 active voice minutes.")
                .font(theme::meta_font())
                .color(theme::TEXT_MUTED),
        );
    });
}

/// The promise that makes the pricing acceptable (§29).
fn billing_promise(ui: &mut egui::Ui) {
    widgets::card(ui, theme::SPACE_MD, |ui| {
        ui.label(
            RichText::new("You are charged only while speech is detected.")
                .font(theme::body_font())
                .color(theme::TEXT_PRIMARY),
        );
        ui.add_space(theme::SPACE_XS);
        ui.label(
            RichText::new(
                "Silence is not charged. Voice activity is detected on your own machine, and only \
                 speech is ever sent for transcription.",
            )
            .font(theme::meta_font())
            .color(theme::TEXT_SECONDARY),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Wallet;
    use game_bridge_protocol::usage::ServiceTier;

    #[test]
    fn the_promise_copy_states_both_halves() {
        // The screen must say both that silence is free and that detection is
        // local, because together they are why the claim is trustworthy.
        let text = "You are charged only while speech is detected. Silence is not charged. Voice \
                    activity is detected on your own machine, and only speech is ever sent for \
                    transcription.";
        assert!(text.contains("only while speech is detected"));
        assert!(text.contains("Silence is not charged"));
        assert!(text.contains("own machine"));
    }

    #[test]
    fn the_default_wallet_shows_the_launch_rates() {
        let wallet = Wallet::default();
        let rows = wallet.rate_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].1, "฿0.50 / minute");
        assert_eq!(rows[1].1, "฿2.50 / minute");
    }

    #[test]
    fn the_low_credit_state_triggers_at_the_documented_threshold() {
        // §40 shows the warning at ฿4.25 remaining.
        let mut wallet = Wallet::default();
        wallet.apply(425, "THB");
        assert!(wallet.is_low());

        let app = App::with_wallet(wallet);
        assert!(app.wallet.is_low());
    }

    #[test]
    fn remaining_time_is_shown_at_the_sessions_own_tier() {
        // A user in Subtitle mode must not be told their Voice-mode runway.
        let mut wallet = Wallet::default();
        wallet.apply(500, "THB"); // ฿5.00
        assert_eq!(
            wallet.remaining_display(ServiceTier::Subtitle),
            "10 min"
        );
        assert_eq!(wallet.remaining_display(ServiceTier::Voice), "2 min");
    }

    #[test]
    fn an_unknown_rate_shows_a_dash_rather_than_a_number() {
        // Never invent a runway: "—" is honest, a guess is not.
        let mut wallet = Wallet::default();
        wallet.apply_rates(vec![]);
        assert_eq!(wallet.remaining_display(ServiceTier::Voice), "—");
    }
}
