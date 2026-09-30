//! Subtitle line buffering, styling, and expiry (§32).
//!
//! Portable and fully tested. The window that renders this lives in
//! [`super::window`].

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::LanguagePair;

/// Lower bound on how long a subtitle stays on screen.
pub const MIN_DURATION_SECS: f32 = 1.0;
/// Upper bound on how long a subtitle stays on screen.
pub const MAX_DURATION_SECS: f32 = 15.0;

/// Which text the overlay shows (§23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextMode {
    /// Only the translation.
    #[default]
    TranslationOnly,
    /// Only the original transcript.
    OriginalOnly,
    /// Both, original above translation.
    Both,
}

impl TextMode {
    /// Label for the feed's mode selector (§23).
    pub const fn label(self) -> &'static str {
        match self {
            TextMode::TranslationOnly => "Translation only",
            TextMode::OriginalOnly => "Original only",
            TextMode::Both => "Both",
        }
    }

    /// Whether the original transcript is shown.
    pub const fn shows_original(self) -> bool {
        matches!(self, TextMode::OriginalOnly | TextMode::Both)
    }

    /// Whether the translation is shown.
    pub const fn shows_translation(self) -> bool {
        matches!(self, TextMode::TranslationOnly | TextMode::Both)
    }

    /// Every mode, in display order.
    pub const ALL: [TextMode; 3] = [
        TextMode::OriginalOnly,
        TextMode::Both,
        TextMode::TranslationOnly,
    ];
}

/// Where the overlay sits on screen (§32).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayPosition {
    /// Bottom centre, above the HUD.
    #[default]
    BottomCenter,
    /// Top centre, below the scoreboard.
    TopCenter,
    /// Above the player's usual field of view.
    BottomLeft,
    /// Bottom right.
    BottomRight,
    /// A specific pixel offset, used when a user drags it.
    Custom {
        /// X offset in pixels from the left of the primary monitor.
        x: i32,
        /// Y offset in pixels from the top of the primary monitor.
        y: i32,
    },
}

/// Overlay appearance and behaviour (§32, §33, §35).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlayStyle {
    /// Where to draw it.
    pub position: OverlayPosition,
    /// Background opacity in `0.0..=1.0`. The text is always fully opaque.
    pub background_opacity: f32,
    /// Font size in points.
    pub font_size: f32,
    /// Which text to show.
    pub text_mode: TextMode,
    /// How long a line stays on screen, in seconds.
    pub duration_secs: f32,
    /// Maximum lines retained on screen at once.
    pub max_lines: usize,
    /// Whether to draw a subtle outline behind the text so it stays legible
    /// against a bright skybox.
    pub text_outline: bool,
}

impl Default for OverlayStyle {
    fn default() -> Self {
        Self {
            position: OverlayPosition::BottomCenter,
            // Dark enough to read against, translucent enough not to blind.
            background_opacity: 0.55,
            font_size: 20.0,
            text_mode: TextMode::Both,
            duration_secs: 4.0,
            max_lines: 3,
            text_outline: true,
        }
    }
}

impl OverlayStyle {
    /// Clamp every field into its supported range.
    ///
    /// Called on load and on every change, because these values come from a
    /// config file a user can edit. A `max_lines` of zero would render nothing
    /// and look like a bug; a `duration_secs` of 600 would leave stale callouts
    /// on screen for ten minutes.
    pub fn sanitize(&mut self) {
        self.background_opacity = self.background_opacity.clamp(0.0, 1.0);
        self.font_size = self.font_size.clamp(10.0, 48.0);
        self.duration_secs = self
            .duration_secs
            .clamp(MIN_DURATION_SECS, MAX_DURATION_SECS);
        self.max_lines = self.max_lines.clamp(1, 8);
    }

    /// Whether this style renders anything at all.
    pub fn is_visible(&self) -> bool {
        self.background_opacity > 0.0 && self.max_lines > 0
    }

    /// The duration as a [`Duration`].
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f32(self.duration_secs.max(0.0))
    }
}

/// One subtitle on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleLine {
    /// Segment this line came from.
    pub segment_id: u64,
    /// The original transcript, once known.
    pub original: Option<String>,
    /// The translated text, once known.
    pub translation: Option<String>,
    /// Language the original was in.
    pub source_language: game_bridge_protocol::Language,
    /// Language of the translation.
    pub target_language: game_bridge_protocol::Language,
    /// When the line was first shown.
    pub shown_at: Instant,
    /// When the line expires. Extended when more text arrives.
    pub expires_at: Instant,
    /// Whether the translation has been finalised.
    pub is_final: bool,
}

impl SubtitleLine {
    /// Create a line that expires `duration` after `now`.
    pub fn new(
        segment_id: u64,
        pair: LanguagePair,
        now: Instant,
        duration: Duration,
    ) -> Self {
        Self {
            segment_id,
            original: None,
            translation: None,
            source_language: pair.source,
            target_language: pair.target,
            shown_at: now,
            expires_at: now + duration,
            is_final: false,
        }
    }

    /// Whether this line has any text to show.
    pub fn has_text(&self) -> bool {
        self.original.as_deref().is_some_and(|t| !t.is_empty())
            || self.translation.as_deref().is_some_and(|t| !t.is_empty())
    }

    /// The lines this entry contributes under `mode`, top to bottom.
    pub fn render_lines(&self, mode: TextMode) -> Vec<(Option<&str>, Option<&str>)> {
        let mut out = Vec::new();
        if mode.shows_original() {
            if let Some(text) = self.original.as_deref().filter(|t| !t.is_empty()) {
                out.push((Some(text), None));
            }
        }
        if mode.shows_translation() {
            if let Some(text) = self.translation.as_deref().filter(|t| !t.is_empty()) {
                out.push((None, Some(text)));
            }
        }
        out
    }

    /// How long until this line expires, or zero if it already has.
    pub fn time_remaining(&self, now: Instant) -> Duration {
        self.expires_at.saturating_duration_since(now)
    }

    /// Whether the line has expired.
    pub fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }
}

/// The overlay's line queue.
///
/// Holds at most `max_lines` entries, newest last, and drops expired ones on
/// [`OverlayModel::tick`].
#[derive(Debug)]
pub struct OverlayModel {
    style: OverlayStyle,
    /// Active lines, oldest first.
    lines: Vec<SubtitleLine>,
    /// Language pair, used to tag new lines.
    pair: LanguagePair,
}

impl OverlayModel {
    /// Create a model with the given style.
    pub fn new(style: OverlayStyle, pair: LanguagePair) -> Self {
        Self {
            style,
            lines: Vec::new(),
            pair,
        }
    }

    /// Current style.
    pub fn style(&self) -> &OverlayStyle {
        &self.style
    }

    /// Replace the style, sanitising it and immediately applying the new line
    /// cap.
    pub fn set_style(&mut self, mut style: OverlayStyle) {
        style.sanitize();
        self.style = style;
        self.enforce_cap();
    }

    /// Active lines, oldest first.
    pub fn lines(&self) -> &[SubtitleLine] {
        &self.lines
    }

    /// Whether anything would be drawn.
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|line| !line.has_text())
    }

    /// Open a line for a segment, or return the existing one.
    pub fn begin(&mut self, segment_id: u64, now: Instant) -> &mut SubtitleLine {
        if let Some(index) = self.lines.iter().position(|l| l.segment_id == segment_id) {
            return &mut self.lines[index];
        }
        let line = SubtitleLine::new(segment_id, self.pair, now, self.style.duration());
        self.lines.push(line);
        self.enforce_cap();
        let index = self.lines.len() - 1;
        &mut self.lines[index]
    }

    /// Apply a transcript, partial or final.
    pub fn set_transcript(&mut self, segment_id: u64, text: impl Into<String>, is_final: bool, now: Instant) {
        let duration = self.style.duration();
        let line = self.begin(segment_id, now);
        line.original = Some(text.into());
        if is_final {
            line.is_final = true;
        }
        self.extend_lifetime(segment_id, now, duration);
    }

    /// Apply a translation, partial or final.
    pub fn set_translation(&mut self, segment_id: u64, text: impl Into<String>, is_final: bool, now: Instant) {
        let duration = self.style.duration();
        let line = self.begin(segment_id, now);
        line.translation = Some(text.into());
        if is_final {
            line.is_final = true;
        }
        self.extend_lifetime(segment_id, now, duration);
    }

    /// Push expiry out for a line that just received more text.
    ///
    /// Without this, a long sentence being transcribed word by word would
    /// expire based on when its first word arrived and vanish mid-sentence.
    fn extend_lifetime(&mut self, segment_id: u64, now: Instant, duration: Duration) {
        if let Some(line) = self.lines.iter_mut().find(|l| l.segment_id == segment_id) {
            line.expires_at = now + duration;
        }
    }

    /// Remove expired lines.
    pub fn tick(&mut self, now: Instant) {
        self.lines.retain(|line| !line.is_expired(now));
    }

    /// Close a segment, marking it final and restarting its expiry clock so the
    /// user gets the full duration to read the finished line.
    pub fn close(&mut self, segment_id: u64, now: Instant) {
        let duration = self.style.duration();
        if let Some(line) = self.lines.iter_mut().find(|l| l.segment_id == segment_id) {
            line.is_final = true;
            line.expires_at = now + duration;
        }
    }

    /// Clear everything, e.g. when a session stops.
    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// Drop the oldest lines until the cap is satisfied.
    fn enforce_cap(&mut self) {
        while self.lines.len() > self.style.max_lines {
            self.lines.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> OverlayModel {
        OverlayModel::new(OverlayStyle::default(), LanguagePair::default())
    }

    fn style_with_max_lines(max_lines: usize) -> OverlayStyle {
        OverlayStyle {
            max_lines,
            ..Default::default()
        }
    }

    #[test]
    fn default_style_sanitizes_to_itself() {
        let mut style = OverlayStyle::default();
        let original = style.clone();
        style.sanitize();
        assert_eq!(style, original);
    }

    #[test]
    fn sanitize_clamps_every_field_into_range() {
        let mut style = OverlayStyle {
            background_opacity: 5.0,
            font_size: 500.0,
            duration_secs: 600.0,
            max_lines: 0,
            ..Default::default()
        };
        style.sanitize();
        assert_eq!(style.background_opacity, 1.0);
        assert_eq!(style.font_size, 48.0);
        assert_eq!(style.duration_secs, MAX_DURATION_SECS);
        assert_eq!(style.max_lines, 1);
    }

    #[test]
    fn sanitize_clamps_negative_values_upward() {
        let mut style = OverlayStyle {
            background_opacity: -1.0,
            font_size: -10.0,
            duration_secs: 0.0,
            max_lines: 99,
            ..Default::default()
        };
        style.sanitize();
        assert_eq!(style.background_opacity, 0.0);
        assert_eq!(style.font_size, 10.0);
        assert_eq!(style.duration_secs, MIN_DURATION_SECS);
        assert_eq!(style.max_lines, 8);
    }

    #[test]
    fn a_fully_transparent_overlay_is_not_visible() {
        let style = OverlayStyle {
            background_opacity: 0.0,
            ..Default::default()
        };
        assert!(!style.is_visible());
    }

    #[test]
    fn text_mode_selects_which_languages_render() {
        let pair = LanguagePair::default();
        let now = Instant::now();
        let mut line = SubtitleLine::new(1, pair, now, Duration::from_secs(4));
        line.original = Some("ศัตรูอยู่ข้างหลัง".into());
        line.translation = Some("Enemy is behind us.".into());

        assert_eq!(line.render_lines(TextMode::OriginalOnly).len(), 1);
        assert_eq!(line.render_lines(TextMode::TranslationOnly).len(), 1);
        assert_eq!(line.render_lines(TextMode::Both).len(), 2);

        assert!(TextMode::Both.shows_original());
        assert!(TextMode::Both.shows_translation());
        assert!(!TextMode::OriginalOnly.shows_translation());
        assert!(!TextMode::TranslationOnly.shows_original());
    }

    #[test]
    fn render_lines_skips_empty_text() {
        let pair = LanguagePair::default();
        let mut line = SubtitleLine::new(1, pair, Instant::now(), Duration::from_secs(4));
        line.original = Some(String::new());
        line.translation = Some("Enemy is behind us.".into());
        let rendered = line.render_lines(TextMode::Both);
        assert_eq!(rendered.len(), 1, "empty original must not render a blank line");
    }

    #[test]
    fn a_line_expires_after_its_duration() {
        // The stale-callout problem: a subtitle that never expires is worse
        // than no subtitle.
        let mut model = model();
        let now = Instant::now();
        model.set_translation(1, "Enemy is behind us.", true, now);
        assert!(!model.is_empty());

        model.tick(now + Duration::from_secs(1));
        assert!(!model.is_empty(), "still within the 4s duration");

        model.tick(now + Duration::from_secs(5));
        assert!(model.is_empty(), "the line should have expired");
        assert!(model.lines().is_empty());
    }

    #[test]
    fn partial_updates_extend_the_lifetime_of_a_line() {
        // A long sentence transcribed word by word must not vanish mid-way.
        let style = OverlayStyle {
            duration_secs: 2.0,
            ..Default::default()
        };
        let mut model = OverlayModel::new(style, LanguagePair::default());
        let start = Instant::now();

        model.set_transcript(1, "ศัตรู", false, start);
        // Three seconds later, still more words arriving.
        let later = start + Duration::from_secs(3);
        model.set_transcript(1, "ศัตรูอยู่ข้างหลัง", false, later);

        // At the original expiry the line is still alive because it was renewed.
        model.tick(start + Duration::from_secs(3));
        assert_eq!(model.lines().len(), 1);

        // And it does expire relative to the last update.
        model.tick(later + Duration::from_secs(3));
        assert!(model.lines().is_empty());
    }

    #[test]
    fn partial_and_final_updates_reuse_the_same_line() {
        let mut model = model();
        let now = Instant::now();
        model.set_transcript(7, "ศัตรู", false, now);
        model.set_transcript(7, "ศัตรูอยู่ข้างหลัง", true, now);
        model.set_translation(7, "Enemy is behind us.", true, now);
        assert_eq!(model.lines().len(), 1, "one segment is one line");
        assert!(model.lines()[0].is_final);
        assert_eq!(model.lines()[0].translation.as_deref(), Some("Enemy is behind us."));
    }

    #[test]
    fn lines_are_capped_at_the_configured_maximum() {
        let mut model = OverlayModel::new(style_with_max_lines(3), LanguagePair::default());
        let now = Instant::now();
        for segment in 0..6 {
            model.set_translation(segment, format!("line {segment}"), true, now);
        }
        assert_eq!(model.lines().len(), 3);
        // The newest lines survive; the oldest are evicted.
        assert_eq!(model.lines()[0].segment_id, 3);
        assert_eq!(model.lines()[2].segment_id, 5);
    }

    #[test]
    fn lowering_the_cap_evicts_immediately() {
        let mut model = model();
        let now = Instant::now();
        for segment in 0..3 {
            model.set_translation(segment, "x", true, now);
        }
        assert_eq!(model.lines().len(), 3);

        model.set_style(style_with_max_lines(1));
        assert_eq!(model.lines().len(), 1);
        assert_eq!(model.lines()[0].segment_id, 2, "the newest line is kept");
    }

    #[test]
    fn set_style_sanitizes_before_applying() {
        let mut model = model();
        model.set_style(OverlayStyle {
            max_lines: 0,
            duration_secs: 999.0,
            ..Default::default()
        });
        assert_eq!(model.style().max_lines, 1);
        assert_eq!(model.style().duration_secs, MAX_DURATION_SECS);
    }

    #[test]
    fn closing_a_segment_gives_the_reader_the_full_duration() {
        let style = OverlayStyle {
            duration_secs: 3.0,
            ..Default::default()
        };
        let mut model = OverlayModel::new(style, LanguagePair::default());
        let start = Instant::now();
        model.set_translation(1, "Enemy is behind us.", false, start);

        // Close two seconds in; the reader should still get a full 3 seconds
        // from the close, not one second of what remained.
        let closed_at = start + Duration::from_secs(2);
        model.close(1, closed_at);
        assert!(model.lines()[0].is_final);

        model.tick(closed_at + Duration::from_secs(2));
        assert_eq!(model.lines().len(), 1, "still readable");

        model.tick(closed_at + Duration::from_secs(4));
        assert!(model.lines().is_empty());
    }

    #[test]
    fn clear_removes_everything() {
        let mut model = model();
        let now = Instant::now();
        model.set_translation(1, "a", true, now);
        model.set_translation(2, "b", true, now);
        model.clear();
        assert!(model.lines().is_empty());
        assert!(model.is_empty());
    }

    #[test]
    fn opening_the_same_segment_twice_does_not_duplicate_it() {
        let mut model = model();
        let now = Instant::now();
        model.begin(5, now);
        model.begin(5, now);
        assert_eq!(model.lines().len(), 1);
    }

    #[test]
    fn an_empty_model_reports_empty() {
        let model = model();
        assert!(model.is_empty());
        assert!(model.lines().is_empty());
    }

    #[test]
    fn a_line_without_text_does_not_count_as_visible() {
        let mut model = model();
        model.begin(1, Instant::now());
        assert!(model.is_empty(), "a line with no text draws nothing");
    }

    #[test]
    fn time_remaining_counts_down_and_floors_at_zero() {
        let pair = LanguagePair::default();
        let now = Instant::now();
        let line = SubtitleLine::new(1, pair, now, Duration::from_secs(4));
        assert_eq!(line.time_remaining(now), Duration::from_secs(4));
        assert_eq!(
            line.time_remaining(now + Duration::from_secs(1)),
            Duration::from_secs(3)
        );
        assert_eq!(line.time_remaining(now + Duration::from_secs(10)), Duration::ZERO);
        assert!(line.is_expired(now + Duration::from_secs(10)));
    }

    #[test]
    fn style_round_trips_through_json() {
        let style = OverlayStyle::default();
        let json = serde_json::to_string(&style).unwrap();
        let back: OverlayStyle = serde_json::from_str(&json).unwrap();
        assert_eq!(back, style);
    }

    #[test]
    fn custom_position_survives_serialization() {
        let style = OverlayStyle {
            position: OverlayPosition::Custom { x: 100, y: 200 },
            ..Default::default()
        };
        let json = serde_json::to_string(&style).unwrap();
        let back: OverlayStyle = serde_json::from_str(&json).unwrap();
        assert_eq!(back.position, OverlayPosition::Custom { x: 100, y: 200 });
    }

    #[test]
    fn every_text_mode_is_offered_in_the_selector() {
        assert_eq!(TextMode::ALL.len(), 3);
        for mode in TextMode::ALL {
            assert!(!mode.label().is_empty());
        }
    }
}
