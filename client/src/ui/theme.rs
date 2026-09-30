//! Design tokens for the Spotify-inspired dark UI (§18, §33, §34, §35, §36).
//!
//! Every colour, radius, spacing step, and type size in the application comes
//! from this module. Screens never hardcode a value.
//!
//! # Why a token module rather than inline literals
//!
//! egui's immediate-mode API makes it trivially easy to write
//! `Color32::from_rgb(24, 24, 24)` at a call site, and trivially hard to find it
//! later. The spec calls for *consistent* surface levels, radii, and type scale
//! (§33, §35, §36), and consistency across a dozen screens is only achievable if
//! the values live in one place that a reviewer can diff.

use egui::{Color32, FontId, Stroke, Vec2};

// ---------------------------------------------------------------------------
// Surface levels (§33)
// ---------------------------------------------------------------------------

/// Base window background. Near-black, as §33 requires, but not pure black:
/// `#000000` against a game's own dark UI reads as a hole in the screen.
pub const BASE: Color32 = Color32::from_rgb(0x0B, 0x0B, 0x0D);

/// Sidebar background, one step above the base so the sidebar reads as a
/// distinct rail without needing a border.
pub const SIDEBAR: Color32 = Color32::from_rgb(0x12, 0x12, 0x15);

/// Default card background.
pub const SURFACE: Color32 = Color32::from_rgb(0x18, 0x18, 0x1C);

/// A raised card, e.g. the hero panel, one step above [`SURFACE`].
pub const SURFACE_RAISED: Color32 = Color32::from_rgb(0x1F, 0x1F, 0x24);

/// Hover state. A subtle brightness increase, never a colour change.
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(0x28, 0x28, 0x2E);

/// Pressed/selected state.
pub const SURFACE_ACTIVE: Color32 = Color32::from_rgb(0x30, 0x30, 0x38);

/// Hairline divider between sections.
pub const DIVIDER: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x30);

/// Input field background, recessed below [`SURFACE`].
pub const INPUT: Color32 = Color32::from_rgb(0x0F, 0x0F, 0x12);

// ---------------------------------------------------------------------------
// Text (§35)
// ---------------------------------------------------------------------------

/// Primary text. Not pure white: `#FFFFFF` on near-black is harsh for long
/// reading and looks cheap next to a game's own UI.
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xF2, 0xF2, 0xF4);

/// Secondary text: labels, metadata.
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0xA6, 0xA6, 0xAE);

/// Tertiary text: timestamps, hints, disabled labels.
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x6E, 0x6E, 0x77);

/// Text on top of an accent-filled button.
pub const TEXT_ON_ACCENT: Color32 = Color32::from_rgb(0x08, 0x0A, 0x0C);

// ---------------------------------------------------------------------------
// Accent and status (§34)
// ---------------------------------------------------------------------------

/// The Game Bridge accent.
///
/// A cyan-leaning teal, deliberately not Spotify's green: §34 asks for a
/// distinct accent so the app is not mistaken for a Spotify product, and green
/// is the single most recognisable part of that identity. Cyan also reads well
/// against a game's warm HUD, where green often collides with health bars and
/// party indicators.
pub const ACCENT: Color32 = Color32::from_rgb(0x35, 0xE0, 0xC8);

/// Accent, dimmed, for pressed states and subtle fills.
pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x1E, 0x8C, 0x7E);

/// Accent at low alpha, for the selected sidebar item's background.
pub const ACCENT_WASH: Color32 = Color32::from_rgba_premultiplied(0x0C, 0x38, 0x32, 0xFF);

/// Live/active indicator. Same as the accent: "live" *is* the accent state.
pub const LIVE: Color32 = ACCENT;

/// Warning, e.g. low credit (§40).
pub const WARNING: Color32 = Color32::from_rgb(0xE8, 0xA5, 0x3A);

/// Error, e.g. a dropped connection or a missing driver (§40).
pub const ERROR: Color32 = Color32::from_rgb(0xE0, 0x5A, 0x5A);

/// Success/connected, used sparingly; most positive states use the accent.
pub const SUCCESS: Color32 = Color32::from_rgb(0x4C, 0xC9, 0x7A);

/// Voice-activity meter fill. Slightly softer than the accent so a constantly
/// moving bar is not distracting.
pub const ACTIVITY: Color32 = Color32::from_rgb(0x54, 0xC8, 0xB4);

// ---------------------------------------------------------------------------
// Radii (§36)
// ---------------------------------------------------------------------------

/// Main content card radius: 12–16px per §36.
pub const RADIUS_CARD: u8 = 14;

/// Small card radius: 8–12px per §36.
pub const RADIUS_SMALL: u8 = 10;

/// Input field radius: 8px per §36.
pub const RADIUS_INPUT: u8 = 8;

/// Pill radius, used for every button.
///
/// `u8::MAX` rather than a measured half-height, because egui clamps the corner
/// radius per axis; this is how a pill is expressed.
pub const RADIUS_PILL: u8 = u8::MAX;

/// Card corner radius as egui's type.
pub const fn card_radius() -> egui::Rounding {
    egui::Rounding::same(RADIUS_CARD as f32)
}

/// Small-card corner radius.
pub const fn small_radius() -> egui::Rounding {
    egui::Rounding::same(RADIUS_SMALL as f32)
}

/// Input corner radius.
pub const fn input_radius() -> egui::Rounding {
    egui::Rounding::same(RADIUS_INPUT as f32)
}

/// Pill corner radius.
pub const fn pill_radius() -> egui::Rounding {
    egui::Rounding::same(RADIUS_PILL as f32)
}

// ---------------------------------------------------------------------------
// Spacing scale
// ---------------------------------------------------------------------------

/// Base spacing unit, in points. Every gap is a multiple of this.
pub const UNIT: f32 = 4.0;

/// Extra-small gap.
pub const SPACE_XS: f32 = UNIT;
/// Small gap.
pub const SPACE_SM: f32 = UNIT * 2.0;
/// Medium gap. The default padding inside a card.
pub const SPACE_MD: f32 = UNIT * 4.0;
/// Large gap between cards.
pub const SPACE_LG: f32 = UNIT * 6.0;
/// Extra-large gap between sections.
pub const SPACE_XL: f32 = UNIT * 8.0;

/// Sidebar width. Fixed, so the content area does not shift between screens.
pub const SIDEBAR_WIDTH: f32 = 216.0;

/// Height of the persistent translation control bar (§30).
pub const MINI_PLAYER_HEIGHT: f32 = 64.0;

// ---------------------------------------------------------------------------
// Typography (§35)
// ---------------------------------------------------------------------------

/// Hero heading, 28–36px per §35. Used for "Good evening" and screen titles.
pub const HERO_SIZE: f32 = 30.0;
/// Section heading, 20–24px.
pub const SECTION_SIZE: f32 = 21.0;
/// Card title, 15–18px.
pub const CARD_TITLE_SIZE: f32 = 16.0;
/// Body text, 13–15px.
pub const BODY_SIZE: f32 = 14.0;
/// Metadata, 11–13px.
pub const META_SIZE: f32 = 12.0;
/// The large language pair in the hero card, e.g. "TH → EN".
pub const LANGUAGE_SIZE: f32 = 22.0;

/// Hero font.
pub fn hero_font() -> FontId {
    FontId::proportional(HERO_SIZE)
}

/// Section heading font.
pub fn section_font() -> FontId {
    FontId::proportional(SECTION_SIZE)
}

/// Card title font.
pub fn card_title_font() -> FontId {
    FontId::proportional(CARD_TITLE_SIZE)
}

/// Body font.
pub fn body_font() -> FontId {
    FontId::proportional(BODY_SIZE)
}

/// Metadata font.
pub fn meta_font() -> FontId {
    FontId::proportional(META_SIZE)
}

/// Language-pair font.
pub fn language_font() -> FontId {
    FontId::proportional(LANGUAGE_SIZE)
}

// ---------------------------------------------------------------------------
// Strokes
// ---------------------------------------------------------------------------

// `Stroke::new` is not a const fn in egui 0.29, so these are functions rather
// than constants. Call sites read the same.

/// Hairline stroke for card outlines. Cards are mostly distinguished by fill;
/// a stroke is used only where a fill difference would be too subtle.
pub fn hairline() -> Stroke {
    Stroke::new(1.0_f32, DIVIDER)
}

/// Stroke for a selected card.
pub fn selected() -> Stroke {
    Stroke::new(1.5_f32, ACCENT)
}

/// Stroke for an error card.
pub fn error_stroke() -> Stroke {
    Stroke::new(1.0_f32, ERROR)
}

/// Stroke for a warning card.
pub fn warning_stroke() -> Stroke {
    Stroke::new(1.0_f32, WARNING)
}

// ---------------------------------------------------------------------------
// Component sizes
// ---------------------------------------------------------------------------

/// Height of a primary pill button. Large enough to be an easy target while
/// playing (§18: "Large primary actions").
pub const BUTTON_HEIGHT: f32 = 42.0;

/// Height of a secondary button.
pub const BUTTON_HEIGHT_SMALL: f32 = 32.0;

/// Diameter of a status dot.
pub const DOT_SIZE: f32 = 8.0;

/// Height of the voice-activity waveform strip (§22: subtle, not a big
/// spectrum analyzer).
pub const WAVEFORM_HEIGHT: f32 = 28.0;

/// Size of the app window at first launch.
pub const DEFAULT_WINDOW_SIZE: Vec2 = Vec2::new(1100.0, 720.0);

/// Minimum window size. Below this the sidebar and content collide.
pub const MIN_WINDOW_SIZE: Vec2 = Vec2::new(880.0, 560.0);

/// Colour for a status dot given a state.
pub fn status_color(state: StatusTone) -> Color32 {
    match state {
        StatusTone::Ready => TEXT_MUTED,
        StatusTone::Live | StatusTone::Connected => LIVE,
        StatusTone::Warning => WARNING,
        StatusTone::Error => ERROR,
    }
}

/// The tone a status indicator should convey (§34).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    /// Idle but healthy. Neutral, not coloured: a coloured "ready" dot competes
    /// with the accent that means "live".
    Ready,
    /// A session is running.
    Live,
    /// Connected, e.g. the gateway link is up.
    Connected,
    /// Something needs the user's attention but is not fatal.
    Warning,
    /// Something is broken.
    Error,
}

impl StatusTone {
    /// The label a status pill shows.
    pub const fn label(self) -> &'static str {
        match self {
            StatusTone::Ready => "Ready",
            StatusTone::Live => "Live",
            StatusTone::Connected => "Connected",
            StatusTone::Warning => "Warning",
            StatusTone::Error => "Error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_levels_increase_in_brightness() {
        // §33 asks for layered surfaces. If a lower level were brighter than a
        // higher one, cards would appear to sink into the background.
        let luma = |c: Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
        assert!(luma(BASE) < luma(SIDEBAR), "sidebar must sit above base");
        assert!(luma(SIDEBAR) < luma(SURFACE), "surface above sidebar");
        assert!(luma(SURFACE) < luma(SURFACE_RAISED), "raised above surface");
        assert!(luma(SURFACE_RAISED) < luma(SURFACE_HOVER), "hover above raised");
        assert!(luma(SURFACE_HOVER) < luma(SURFACE_ACTIVE), "active above hover");
    }

    #[test]
    fn the_base_background_is_near_black_but_not_black() {
        // §33 says "near-black". Pure black on an OLED panel next to a game's
        // own UI reads as a hole rather than a surface.
        assert!(BASE.r() > 0, "base must not be pure black");
        assert!(BASE.r() < 0x20, "base must be near-black");
    }

    #[test]
    fn text_contrast_is_sufficient_against_its_background() {
        // A rough but real check: the primary text must be much brighter than
        // the surfaces it sits on, or the app is unreadable on a bright monitor.
        let luma = |c: Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
        assert!(
            luma(TEXT_PRIMARY) > luma(SURFACE_ACTIVE) * 3,
            "primary text must stand well clear of the lightest surface"
        );
        assert!(luma(TEXT_PRIMARY) > luma(TEXT_SECONDARY));
        assert!(luma(TEXT_SECONDARY) > luma(TEXT_MUTED));
    }

    /// Hue in degrees, for perceptual comparison. RGB distance is a poor proxy
    /// for "does this look like that colour": two greens can be far apart in RGB
    /// and still read as the same brand colour.
    fn hue(color: Color32) -> f32 {
        let r = f32::from(color.r()) / 255.0;
        let g = f32::from(color.g()) / 255.0;
        let b = f32::from(color.b()) / 255.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        if delta == 0.0 {
            return 0.0;
        }
        let hue = if max == r {
            60.0 * (((g - b) / delta) % 6.0)
        } else if max == g {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        if hue < 0.0 {
            hue + 360.0
        } else {
            hue
        }
    }

    #[test]
    fn the_accent_is_perceptually_distinct_from_spotify_green() {
        // §34: the accent must be Game Bridge's own, so the app is not mistaken
        // for a Spotify product. Spotify's green, #1DB954, sits at roughly 142
        // degrees of hue.
        let spotify = Color32::from_rgb(0x1D, 0xB9, 0x54);
        let ours = hue(ACCENT);
        let theirs = hue(spotify);
        let separation = (ours - theirs).abs();

        assert!(
            separation > 20.0,
            "accent hue {ours:.0}° is only {separation:.0}° from Spotify green {theirs:.0}°"
        );
        // And it must read as a different family entirely: ours is teal, which
        // puts it past 160 degrees.
        assert!(ours > 160.0, "accent hue is {ours:.0}°, expected teal");
    }

    #[test]
    fn the_accent_is_dominant_in_cyan_teal() {
        // A documentary assertion: the accent is not red or yellow, which would
        // collide with damage and warning indicators in most games' HUDs.
        assert!(ACCENT.g() > ACCENT.r(), "accent should be cool-toned");
        assert!(ACCENT.b() > ACCENT.r());
    }

    #[test]
    fn error_and_warning_are_distinguishable() {
        assert_ne!(ERROR, WARNING);
        // Warning leans warm-yellow, error leans red: they must not be
        // confusable at a glance, which is the whole point of §34.
        assert!(WARNING.r() > WARNING.b());
        assert!(ERROR.r() > ERROR.g());
        assert!(WARNING.g() > ERROR.g());
    }

    #[test]
    fn status_tones_map_to_the_documented_colours() {
        assert_eq!(status_color(StatusTone::Live), LIVE);
        assert_eq!(status_color(StatusTone::Error), ERROR);
        assert_eq!(status_color(StatusTone::Warning), WARNING);
        // Ready is deliberately neutral rather than coloured.
        assert_eq!(status_color(StatusTone::Ready), TEXT_MUTED);
        assert_ne!(status_color(StatusTone::Ready), LIVE);
    }

    #[test]
    fn status_labels_match_the_spec_screens() {
        // §21 shows "READY"; §22 shows "LIVE"; §19 shows "Connected".
        assert_eq!(StatusTone::Ready.label(), "Ready");
        assert_eq!(StatusTone::Live.label(), "Live");
        assert_eq!(StatusTone::Connected.label(), "Connected");
    }

    #[test]
    fn radii_fall_within_the_spec_ranges() {
        // §36: main card 12–16, input 8, small card 8–12.
        assert!((12..=16).contains(&RADIUS_CARD));
        assert!((8..=12).contains(&RADIUS_SMALL));
        assert_eq!(RADIUS_INPUT, 8);
    }

    #[test]
    fn the_type_scale_is_monotonically_decreasing() {
        // §35 lists hero 28–36, section 20–24, title 15–18, body 13–15,
        // metadata 11–13. Each step must be smaller than the one above it.
        assert!((28.0..=36.0).contains(&HERO_SIZE), "hero {HERO_SIZE}");
        assert!((20.0..=24.0).contains(&SECTION_SIZE), "section {SECTION_SIZE}");
        assert!(
            (15.0..=18.0).contains(&CARD_TITLE_SIZE),
            "title {CARD_TITLE_SIZE}"
        );
        assert!((13.0..=15.0).contains(&BODY_SIZE), "body {BODY_SIZE}");
        assert!((11.0..=13.0).contains(&META_SIZE), "meta {META_SIZE}");

        assert!(HERO_SIZE > SECTION_SIZE);
        assert!(SECTION_SIZE > CARD_TITLE_SIZE);
        assert!(CARD_TITLE_SIZE > BODY_SIZE);
        assert!(BODY_SIZE > META_SIZE);
    }

    #[test]
    fn every_font_uses_its_scale_size() {
        assert_eq!(hero_font().size, HERO_SIZE);
        assert_eq!(section_font().size, SECTION_SIZE);
        assert_eq!(card_title_font().size, CARD_TITLE_SIZE);
        assert_eq!(body_font().size, BODY_SIZE);
        assert_eq!(meta_font().size, META_SIZE);
        assert_eq!(language_font().size, LANGUAGE_SIZE);
    }

    #[test]
    fn spacing_steps_are_ordered_and_multiples_of_the_unit() {
        assert!(SPACE_XS < SPACE_SM);
        assert!(SPACE_SM < SPACE_MD);
        assert!(SPACE_MD < SPACE_LG);
        assert!(SPACE_LG < SPACE_XL);
        for space in [SPACE_XS, SPACE_SM, SPACE_MD, SPACE_LG, SPACE_XL] {
            assert_eq!(space % UNIT, 0.0, "{space} is not a multiple of {UNIT}");
        }
    }

    #[test]
    fn the_minimum_window_is_smaller_than_the_default() {
        // Otherwise the app launches below its own minimum.
        assert!(MIN_WINDOW_SIZE.x < DEFAULT_WINDOW_SIZE.x);
        assert!(MIN_WINDOW_SIZE.y < DEFAULT_WINDOW_SIZE.y);
    }

    #[test]
    fn the_minimum_window_fits_the_sidebar_and_content() {
        // The sidebar plus a workable content column must fit inside the
        // minimum, or the layout collides at its smallest allowed size.
        assert!(
            SIDEBAR_WIDTH + 400.0 <= MIN_WINDOW_SIZE.x,
            "sidebar {SIDEBAR_WIDTH} leaves too little room in a {}-wide window",
            MIN_WINDOW_SIZE.x
        );
    }
}
