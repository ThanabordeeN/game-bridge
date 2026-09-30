//! The application window, sidebar, and persistent control bar.

use std::time::Instant;

use crate::config::ClientConfig;
use crate::overlay::subtitle::{OverlayModel, TextMode};
use crate::routing::bypass::BypassState;
use crate::session::session::{Session, SessionState};
use crate::session::usage::UsageHistory;
use crate::session::Wallet;
use crate::incoming::{IncomingResult, IncomingWorker};
use crate::transcribe::{Deepgram, TranscriptionProvider};
use crate::translate::{IncomingTranslator, TranslatorStats};
use game_bridge_protocol::mode::RoutingMode;
use std::sync::Arc;

use super::screens;
use super::theme;
use super::widgets;

/// Which screen the content area is showing (§20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    /// The start screen with the hero card (§21).
    #[default]
    Home,
    /// Language pair and routing mode (§27).
    Translation,
    /// Voice selection and cloning (§24, §25).
    Voice,
    /// Device selection (§26).
    Audio,
    /// Session history (§28).
    Usage,
    /// Wallet and rates (§29).
    Billing,
    /// Hotkeys, overlay, advanced provider settings (§37, §46).
    Settings,
}

impl Screen {
    /// The label shown in the sidebar.
    pub const fn label(self) -> &'static str {
        match self {
            Screen::Home => "Home",
            Screen::Translation => "Translation",
            Screen::Voice => "Voice",
            Screen::Audio => "Audio",
            Screen::Usage => "Usage",
            Screen::Billing => "Billing",
            Screen::Settings => "Settings",
        }
    }

    /// The sidebar group this screen belongs to (§20).
    pub const fn group(self) -> ScreenGroup {
        match self {
            Screen::Home | Screen::Translation | Screen::Voice | Screen::Audio => {
                ScreenGroup::Primary
            }
            Screen::Usage | Screen::Billing | Screen::Settings => ScreenGroup::Secondary,
        }
    }

    /// The primary group, in sidebar order.
    pub const PRIMARY: [Screen; 4] = [
        Screen::Home,
        Screen::Translation,
        Screen::Voice,
        Screen::Audio,
    ];

    /// The secondary group, in sidebar order.
    pub const SECONDARY: [Screen; 3] = [Screen::Usage, Screen::Billing, Screen::Settings];
}

/// Sidebar sections, which §20 separates with a divider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenGroup {
    /// Home, Translation, Voice, Audio.
    Primary,
    /// Usage, Billing, Settings.
    Secondary,
}

/// The whole application state.
pub struct GameBridgeApp {
    /// Which screen is visible.
    pub screen: Screen,
    /// Persisted configuration.
    pub config: ClientConfig,
    /// The current session.
    pub session: Session,
    /// Wallet balance and rates.
    pub wallet: Wallet,
    /// Session history for the Usage screen.
    pub usage_history: UsageHistory,
    /// Rolling voice-activity levels for the waveform (§22), newest last.
    pub activity: Vec<f32>,
    /// Translation feed for the Translation screen (§23), oldest first.
    pub feed: TranslationFeed,
    /// Subtitle lines, the same model the overlay window renders (§32).
    pub subtitle: OverlayModel,
    /// Text the user typed or pasted to translate, standing in for STT until
    /// audio capture exists.
    pub translation_input: String,
    /// The running incoming worker, when one has been started.
    worker: Option<IncomingWorker>,
    /// Why the worker could not start, if it could not.
    pub translator_error: Option<String>,
    /// Running translation counters, mirrored from the worker.
    pub translation_stats: TranslatorStats,
    /// Description of the active provider, for the status line.
    pub translator_description: Option<String>,
    /// Monotonic segment counter for manually entered text.
    next_manual_segment: u64,
    /// Path of an audio file to transcribe and translate.
    ///
    /// Stands in for captured game audio until WASAPI loopback exists; the code
    /// path below it is identical, because both end at
    /// [`IncomingWorker::submit_audio`].
    pub audio_path: String,
    /// Whether speech-to-text is configured, for the status line.
    pub transcription_description: Option<String>,
    /// An audio job currently being transcribed, shown as a working indicator
    /// rather than as a feed line, because one job produces several lines.
    pub pending_audio: Option<String>,
    /// Milliseconds of active voice the meter has counted this session.
    pub metered_speech_ms: u64,
    /// Whether the app is in offline demo mode, driven by the in-memory
    /// transport. Lets the entire UI be exercised without a gateway.
    pub demo_mode: bool,
    /// The most recent error worth showing the user (§40).
    pub last_error: Option<String>,
    /// A transient confirmation message.
    pub toast: Option<String>,
}

/// How many bars the voice-activity strip shows.
const ACTIVITY_BARS: usize = 48;

impl GameBridgeApp {
    /// Build the application from configuration.
    pub fn new(config: ClientConfig) -> Self {
        let session = Session::new(config.session.clone());
        let overlay_style = config.overlay.clone();
        let pair = config.session.pair;
        Self {
            screen: Screen::default(),
            config,
            session,
            wallet: Wallet::default(),
            usage_history: UsageHistory::default(),
            activity: vec![0.0; ACTIVITY_BARS],
            feed: TranslationFeed::default(),
            subtitle: OverlayModel::new(overlay_style, pair),
            translation_input: String::new(),
            worker: None,
            translator_error: None,
            translation_stats: TranslatorStats::default(),
            translator_description: None,
            next_manual_segment: 0,
            audio_path: String::new(),
            transcription_description: None,
            pending_audio: None,
            metered_speech_ms: 0,
            demo_mode: false,
            last_error: None,
            toast: None,
        }
    }

    /// The status tone for the current state, used by the sidebar dot and the
    /// hero card (§21, §22, §34).
    pub fn status_tone(&self) -> theme::StatusTone {
        match self.session.state() {
            SessionState::Idle | SessionState::Stopped => theme::StatusTone::Ready,
            SessionState::Connecting | SessionState::Reconnecting { .. } => {
                theme::StatusTone::Warning
            }
            SessionState::Live => theme::StatusTone::Live,
            SessionState::Failed { .. } => theme::StatusTone::Error,
        }
    }

    /// Whether the virtual microphone is usable, which drives the §40 error.
    pub fn virtual_mic_ready(&self) -> bool {
        self.config.session.virtual_mic_id.is_some()
    }

    /// Whether starting a session would be refused, and why.
    pub fn start_blocker(&self) -> Option<String> {
        if let Err(problem) = self.config.validate() {
            return Some(problem);
        }
        // Subtitle mode is the only mode that does not need the driver; in the
        // other two the user's voice has nowhere to go without it.
        if self.config.session.mode.needs_virtual_mic_or_voice() && !self.virtual_mic_ready() {
            return Some(
                "Game Bridge Microphone is not installed. Translated voice needs it, or choose \
                 Subtitle mode to continue without voice output."
                    .to_string(),
            );
        }
        if self.wallet.is_exhausted() {
            return Some("Your credit balance is empty. Add credits to start.".to_string());
        }
        None
    }

    /// Begin a session, or record why it could not start.
    pub fn start_session(&mut self) {
        if let Some(blocker) = self.start_blocker() {
            self.last_error = Some(blocker);
            return;
        }
        self.last_error = None;

        // Rebuild the session from the current configuration. `Session`
        // captures its billing tier and device layout at construction, so a
        // session object created before the user changed mode or language would
        // otherwise run — and bill — under the old settings. Rebuilding makes
        // "what is configured" and "what is running" the same thing by
        // construction rather than by convention.
        if self.session.state().is_stopped() {
            self.session = Session::new(self.config.session.clone());
        }

        match self.session.start() {
            Ok(()) => {
                // In demo mode there is no gateway, so complete the handshake
                // locally and let the UI exercise its live states.
                if self.demo_mode {
                    let _ = self.session.on_connected(self.wallet.rates().to_vec());
                }
            }
            Err(error) => self.last_error = Some(error.to_string()),
        }
    }

    /// Stop the session and file the record.
    pub fn stop_session(&mut self) {
        let application = self
            .config
            .session
            .application
            .clone()
            .unwrap_or_else(|| "Game Bridge".to_string());
        self.stop_worker();
        if self.session.stop().is_ok() {
            self.usage_history.push(crate::session::usage::UsageRecord {
                application,
                mode: self.config.session.mode,
                tier: self.session.usage().tier,
                active_voice: self.session.usage().active_voice,
                cost_minor: self.session.usage().cost_minor,
                currency: self.wallet.currency().to_string(),
                started_at_unix: crate::network::auth::now_unix(),
            });
        }
    }

    /// Toggle the bypass latch (§12, §30).
    pub fn toggle_bypass(&mut self) {
        self.session.toggle_bypass();
    }

    /// The current bypass state.
    pub fn bypass(&self) -> BypassState {
        self.session.bypass()
    }

    /// Push a new activity level and age out the oldest.
    pub fn push_activity(&mut self, level: f32) {
        if self.activity.len() >= ACTIVITY_BARS {
            self.activity.remove(0);
        }
        self.activity.push(level.clamp(0.0, 1.0));
    }

    /// Append a line to the feed.
    pub fn push_feed_line(&mut self, line: FeedLine) {
        self.feed.push(line);
    }

    /// The translation feed, newest first (§23).
    ///
    /// Reversed for display: §23 shows the newest line at the top.
    pub fn session_feed(&self) -> Vec<&FeedLine> {
        self.feed.newest_first()
    }

    /// Clear the feed, e.g. when a session stops.
    pub fn clear_feed(&mut self) {
        self.feed.clear();
    }

    // -- incoming translation ------------------------------------------------

    /// Build the provider described by the configuration.
    ///
    /// Returns the provider and its description, or an error explaining why it
    /// could not be built. Kept separate from the worker so it can be exercised
    /// without starting a thread.
    fn build_provider(
        &self,
    ) -> Result<(Arc<dyn crate::translate::TranslationProvider>, String), String> {
        use crate::config::ProviderMode;

        match self.config.provider.mode {
            ProviderMode::Demo => {
                let provider = crate::translate::DemoTranslator::new(
                    // Incoming direction: teammates speak the pair's target.
                    self.config.session.pair.target,
                    self.config.session.pair.source,
                );
                let described = crate::translate::TranslationProvider::describe(&provider);
                Ok((Arc::new(provider), described))
            }
            ProviderMode::OpenAi => {
                let openai = self.config.provider.to_openai_config();
                // A missing key is not fatal: a local server needs none, and the
                // provider's own error is more specific than a guess here.
                let key = crate::translate::ApiKey::from_env(&self.config.provider.api_key_env);
                let provider = crate::translate::OpenAiCompatible::new(openai, key)
                    .map_err(|error| error.user_message())?;
                let described = crate::translate::TranslationProvider::describe(&provider);
                Ok((Arc::new(provider), described))
            }
        }
    }

    /// Whether an API key was found for the configured environment variable.
    pub fn api_key_present(&self) -> bool {
        crate::translate::ApiKey::from_env(&self.config.provider.api_key_env).is_some()
    }

    /// Whether a speech-to-text key was found.
    pub fn transcription_key_present(&self) -> bool {
        crate::secret::Secret::from_env(&self.config.transcription.api_key_env).is_some()
    }

    /// Build the speech-to-text provider, when one is configured.
    ///
    /// Returns `None` rather than an error when transcription is switched off
    /// or its key is absent: text translation still works without it, and
    /// refusing to start the whole pipeline would be the wrong trade.
    fn build_transcriber(&self) -> Option<Arc<dyn TranscriptionProvider>> {
        if !self.config.transcription.enabled {
            return None;
        }
        let key = crate::secret::Secret::from_env(&self.config.transcription.api_key_env);
        if key.is_none() {
            return None;
        }
        let config = self.config.transcription.to_deepgram_config();
        match Deepgram::new(config, key) {
            Ok(provider) => Some(Arc::new(provider)),
            Err(error) => {
                tracing::warn!("speech-to-text unavailable: {}", error.user_message());
                None
            }
        }
    }

    /// Start the translation worker if it is not already running.
    ///
    /// Called lazily, on the first translation, so a user who never uses the
    /// incoming path never pays for a provider or a thread.
    pub fn ensure_worker(&mut self) -> bool {
        if self.worker.as_ref().is_some_and(|w| w.is_alive()) {
            return true;
        }

        let (provider, described) = match self.build_provider() {
            Ok(pair) => pair,
            Err(message) => {
                self.translator_error = Some(message);
                return false;
            }
        };

        let translator = IncomingTranslator::new(provider, self.config.session.pair)
            .with_context(self.config.session.context.clone());

        let transcriber = self.build_transcriber();
        self.transcription_description = transcriber
            .as_ref()
            .map(|provider| provider.describe());

        self.worker = Some(IncomingWorker::spawn(transcriber, translator));
        self.translator_description = Some(described);
        self.translator_error = None;
        true
    }

    /// Stop the worker, e.g. when a session stops or settings change.
    pub fn stop_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.stop();
        }
        self.translator_description = None;
    }

    /// Queue a transcript for translation.
    ///
    /// This is the entry point the STT layer will call once audio capture
    /// exists; today the Translation screen calls it with typed text.
    pub fn submit_transcript(&mut self, segment_id: u64, text: impl Into<String>) -> bool {
        let text = text.into();
        if text.trim().is_empty() {
            return false;
        }
        if !self.ensure_worker() {
            return false;
        }
        let submitted = self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.submit_text(segment_id, text.clone()));

        if !submitted {
            self.translator_error = Some(
                "The translation worker stopped. Reopen the provider settings to restart it."
                    .to_string(),
            );
            return false;
        }

        // Show the original immediately, so the reader sees the callout while
        // the translation is still in flight rather than an empty panel.
        let now = Instant::now();
        self.subtitle
            .set_transcript(segment_id, text.clone(), false, now);
        self.feed
            .push_original(segment_id, text, self.config.session.pair);
        true
    }

    /// Queue the text currently in the input box.
    pub fn submit_translation_input(&mut self) -> bool {
        let text = std::mem::take(&mut self.translation_input);
        if text.trim().is_empty() {
            self.translation_input = text;
            return false;
        }
        let segment_id = self.next_manual_segment;
        self.next_manual_segment += 1;
        self.submit_transcript(segment_id, text)
    }

    /// Queue an audio file to transcribe and then translate.
    ///
    /// The whole chain, on the worker thread. Until WASAPI loopback exists this
    /// is how audio reaches the pipeline; the path below it is the same one
    /// captured game audio will take.
    pub fn submit_audio_file(&mut self, path: &str) -> bool {
        let path = path.trim();
        if path.is_empty() {
            return false;
        }

        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(error) => {
                self.translator_error = Some(format!("Cannot read {path}: {error}"));
                return false;
            }
        };
        if data.is_empty() {
            self.translator_error = Some(format!("{path} is empty."));
            return false;
        }

        if !self.ensure_worker() {
            return false;
        }

        let segment_id = self.next_manual_segment;
        self.next_manual_segment += 1;
        let content_type = audio_content_type(path);

        let submitted = self.worker.as_ref().is_some_and(|worker| {
            worker.submit_audio(segment_id, data, content_type)
        });

        if !submitted {
            self.translator_error = Some(
                "The incoming worker stopped. Reopen the provider settings to restart it."
                    .to_string(),
            );
            return false;
        }

        // One audio job produces one feed line per utterance, so a placeholder
        // line would be wrong. The panel shows a working indicator instead.
        self.pending_audio = Some(file_label(path).to_string());
        true
    }

    /// Queue the audio path currently in the input box.
    pub fn submit_audio_input(&mut self) -> bool {
        let path = std::mem::take(&mut self.audio_path);
        if path.trim().is_empty() {
            self.audio_path = path;
            return false;
        }
        self.submit_audio_file(&path)
    }

    /// Collect finished translations and route them to the feed and subtitles.
    ///
    /// Called once per frame. Non-blocking by construction: the worker owns the
    /// blocking call, and this only drains what has already finished.
    pub fn poll_translations(&mut self) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        let results = worker.drain();
        if results.is_empty() {
            return;
        }

        let now = Instant::now();
        for result in results {
            self.translation_stats = result.stats;
            self.apply_incoming(result, now);
        }
    }

    /// Route one pipeline result to the subtitle model, the feed, and the meter.
    fn apply_incoming(&mut self, result: IncomingResult, now: Instant) {
        let segment_id = result.segment_id;

        // The audio job has produced its first result, so the working indicator
        // has served its purpose.
        self.pending_audio = None;

        // Bill only what the provider actually transcribed. The VAD decides
        // what to upload; the transcript decides what to charge for. Measured
        // against a real clip, the detector called a music intro speech and the
        // provider returned nothing for it — charging on the detector's opinion
        // would bill the user for their game's soundtrack.
        let billable = result.billable_ms();
        if billable > 0 {
            self.session.record_speech(billable);
            self.metered_speech_ms = self.session.usage().active_voice.speech_ms;
        }

        // Transcription failed outright: there is no text at all, so the line
        // is marked rather than left looking like a pending translation.
        if let Some(error) = &result.transcription_error {
            self.feed.set_failure(
                segment_id,
                error.user_message(),
                self.config.session.pair,
            );
            self.subtitle.close(segment_id, now);
            if error.needs_user_action() {
                self.translator_error = Some(error.user_message());
            }
            return;
        }

        // Silence and noise are the normal case in game audio. Not translating
        // them is correct, not a failure, and nothing is billed.
        if result.is_skipped() {
            self.subtitle.close(segment_id, now);
            return;
        }

        if let Some(original) = result.original() {
            if !original.trim().is_empty() {
                self.subtitle
                    .set_transcript(segment_id, original.to_string(), true, now);
                self.feed
                    .push_original(segment_id, original.to_string(), self.config.session.pair);
            }
        }

        if let Some(outcome) = result.outcome {
            self.apply_translation(outcome, now);
        }
    }

    /// Route one translation outcome to the subtitle model and the feed.
    fn apply_translation(&mut self, outcome: crate::translate::TranslationOutcome, now: Instant) {
        let segment_id = outcome.segment_id;

        match (&outcome.translation, &outcome.error) {
            (Some(translated), _) => {
                self.subtitle
                    .set_translation(segment_id, translated.clone(), true, now);
                self.subtitle.close(segment_id, now);
                self.feed.set_translation(
                    segment_id,
                    translated.clone(),
                    self.config.session.pair,
                );
            }
            (None, Some(error)) => {
                // The original stays on screen. §40: say what happened, but do
                // not blank the reader's information.
                self.subtitle.close(segment_id, now);
                self.feed.set_failure(
                    segment_id,
                    error.user_message(),
                    self.config.session.pair,
                );

                // Only actionable failures raise a banner. A transient timeout
                // is already visible in the feed line and in the failure count;
                // a dialog for every one of them would be worse than the
                // timeout itself.
                if error.needs_user_action() {
                    self.translator_error = Some(error.user_message());
                }
            }
            (None, None) => {
                // Skipped, nothing to do.
            }
        }
    }

    /// Whether translation is currently running.
    pub fn translator_running(&self) -> bool {
        self.worker.as_ref().is_some_and(|worker| worker.is_alive())
    }

    /// Age out expired subtitles.
    pub fn tick_subtitles(&mut self, now: Instant) {
        self.subtitle.tick(now);
    }
}

/// One line in the translation feed (§23).
#[derive(Debug, Clone, PartialEq)]
pub struct FeedLine {
    /// Local time as `"19:42"`.
    pub timestamp: String,
    /// The original transcript, if shown.
    pub original: Option<String>,
    /// The translation, if shown.
    pub translation: Option<String>,
    /// Why the translation failed, if it did.
    ///
    /// Distinguishes "still in flight" from "failed". Without it a failed line
    /// renders as "translating…" forever, which is a lie the reader cannot
    /// detect.
    pub error: Option<String>,
    /// Short tag for the original language, e.g. `"TH"`.
    pub source_tag: String,
    /// Short tag for the translation language.
    pub target_tag: String,
    /// Segment this line came from.
    pub segment_id: u64,
}

impl FeedLine {
    /// Build a line from a segment and the user's language pair.
    ///
    /// The tags are the **incoming** direction: the original is what teammates
    /// said, in `pair.target`, and the translation is what the user reads, in
    /// `pair.source`. The direction is taken from
    /// [`crate::translate::incoming_source`] rather than restated here, so the
    /// feed, the subtitle overlay, and the provider request cannot disagree
    /// about which language is which.
    pub fn new(segment_id: u64, pair: crate::LanguagePair, timestamp: impl Into<String>) -> Self {
        Self {
            timestamp: timestamp.into(),
            original: None,
            translation: None,
            error: None,
            source_tag: crate::translate::incoming_source(pair).short_tag(),
            target_tag: crate::translate::incoming_target(pair).short_tag(),
            segment_id,
        }
    }

    /// Whether this line has anything to show.
    pub fn has_content(&self) -> bool {
        self.original.is_some() || self.translation.is_some()
    }

    /// Whether the translation is still in flight.
    pub fn is_pending(&self) -> bool {
        self.translation.is_none() && self.error.is_none()
    }

    /// Whether the translation failed and the original is being shown instead.
    pub fn is_failed(&self) -> bool {
        self.translation.is_none() && self.error.is_some()
    }
}

/// The rolling translation feed.
///
/// Keyed by segment, so a translation arriving after its transcript updates the
/// existing line rather than appending a second one. Treating the two arrivals
/// as independent events is what produces a doubled feed where every callout
/// appears once in the original language and again in the translation.
#[derive(Debug, Clone)]
pub struct TranslationFeed {
    lines: Vec<FeedLine>,
}

impl Default for TranslationFeed {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
        }
    }
}

impl TranslationFeed {
    /// Record a transcript, opening a line for the segment.
    pub fn push_original(
        &mut self,
        segment_id: u64,
        text: impl Into<String>,
        pair: crate::LanguagePair,
    ) {
        if let Some(existing) = self.find_mut(segment_id) {
            existing.original = Some(text.into());
            return;
        }
        let mut line = FeedLine::new(segment_id, pair, format_clock_local());
        line.original = Some(text.into());
        self.push(line);
    }

    /// Attach a translation to its segment's line.
    pub fn set_translation(
        &mut self,
        segment_id: u64,
        text: impl Into<String>,
        pair: crate::LanguagePair,
    ) {
        if let Some(existing) = self.find_mut(segment_id) {
            existing.translation = Some(text.into());
            return;
        }
        let mut line = FeedLine::new(segment_id, pair, format_clock_local());
        line.translation = Some(text.into());
        self.push(line);
    }

    /// Record that a segment's translation failed.
    pub fn set_failure(
        &mut self,
        segment_id: u64,
        message: impl Into<String>,
        pair: crate::LanguagePair,
    ) {
        if let Some(existing) = self.find_mut(segment_id) {
            existing.error = Some(message.into());
            return;
        }
        let mut line = FeedLine::new(segment_id, pair, format_clock_local());
        line.error = Some(message.into());
        self.push(line);
    }

    /// Append a line, evicting the oldest at capacity.
    pub fn push(&mut self, line: FeedLine) {
        if self.lines.len() >= FEED_CAPACITY {
            self.lines.remove(0);
        }
        self.lines.push(line);
    }

    fn find_mut(&mut self, segment_id: u64) -> Option<&mut FeedLine> {
        self.lines
            .iter_mut()
            .rev()
            .find(|line| line.segment_id == segment_id)
    }

    /// Lines, oldest first.
    pub fn lines(&self) -> &[FeedLine] {
        &self.lines
    }

    /// Lines, newest first, as §23 displays them.
    pub fn newest_first(&self) -> Vec<&FeedLine> {
        self.lines.iter().rev().collect()
    }

    /// Number of lines.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether the feed is empty.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Remove every line.
    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// The most recent line.
    pub fn latest(&self) -> Option<&FeedLine> {
        self.lines.last()
    }
}

/// The current local time as `"19:42"`, for feed timestamps.
///
/// Falls back to UTC when the local offset cannot be determined, which happens
/// on some platforms inside a multithreaded process. A UTC timestamp is wrong
/// for most users by a few hours, which is cosmetic; failing to render the line
/// at all would not be.
pub fn format_clock_local() -> String {
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    format_clock_at(std::time::SystemTime::now(), offset)
}

/// Format a time with an explicit offset.
///
/// Split out from [`format_clock_local`] so the formatting is testable without
/// depending on the machine's timezone.
pub fn format_clock_at(now: std::time::SystemTime, offset: time::UtcOffset) -> String {
    let Ok(datetime) = time::OffsetDateTime::from_unix_timestamp(
        now.duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    ) else {
        return "--:--".to_string();
    };
    let local = datetime.to_offset(offset);
    format!("{:02}:{:02}", local.hour(), local.minute())
}

/// Guess an audio container type from a file extension.
///
/// The provider needs the container, not just the bytes, and a wrong guess is
/// rejected rather than mis-decoded.
pub fn audio_content_type(path: &str) -> &'static str {
    let lowered = path.to_lowercase();
    if lowered.ends_with(".wav") {
        "audio/wav"
    } else if lowered.ends_with(".mp3") {
        "audio/mpeg"
    } else if lowered.ends_with(".ogg") {
        "audio/ogg"
    } else if lowered.ends_with(".flac") {
        "audio/flac"
    } else if lowered.ends_with(".m4a") {
        "audio/mp4"
    } else if lowered.ends_with(".webm") {
        "audio/webm"
    } else {
        // Raw 16-bit little-endian PCM, which is what the capture path produces.
        "audio/l16"
    }
}

/// The final path component, for display.
pub fn file_label(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// How many feed lines are retained.
const FEED_CAPACITY: usize = 200;

impl Default for GameBridgeApp {
    fn default() -> Self {
        Self::new(ClientConfig::default())
    }
}

impl GameBridgeApp {
    /// An app in offline demo mode, for screenshots and UI work without a
    /// gateway.
    pub fn demo() -> Self {
        Self {
            demo_mode: true,
            ..Self::default()
        }
    }

    /// An app with a specific wallet, so the Billing screen can be exercised
    /// against a given balance.
    pub fn with_wallet(wallet: Wallet) -> Self {
        Self {
            wallet,
            ..Self::default()
        }
    }
}

/// Launch the desktop application.
#[cfg(feature = "gui")]
pub fn run() -> anyhow::Result<()> {
    use tracing_subscriber::EnvFilter;

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config_path = ClientConfig::default_path();
    let config = match ClientConfig::load_or_default(&config_path) {
        Ok(config) => config,
        Err(error) => {
            // A corrupt config must not prevent the app from starting: the user
            // needs the UI in order to fix it.
            tracing::warn!("could not load config from {}: {error}", config_path.display());
            ClientConfig::default()
        }
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(theme::DEFAULT_WINDOW_SIZE)
            .with_min_inner_size(theme::MIN_WINDOW_SIZE)
            .with_title("Game Bridge"),
        ..Default::default()
    };

    eframe::run_native(
        "Game Bridge",
        options,
        Box::new(|_cc| Ok(Box::new(GameBridgeApp::new(config)))),
    )
    .map_err(|e| anyhow::anyhow!("failed to start the Game Bridge window: {e}"))
}

impl eframe::App for GameBridgeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        self.session.tick(now);

        // Collect finished translations and expire stale subtitles before
        // drawing, so the frame renders the current state rather than the
        // previous one.
        self.poll_translations();
        self.tick_subtitles(now);

        // The UI is repainted a few times a second while live so the clock and
        // the activity strip animate, and only on demand when idle so an idle
        // client costs ~0% CPU (§41).
        if self.session.state().is_live() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else if self.translator_running() && self.feed.latest().is_some_and(|l| l.is_pending()) {
            // A translation is in flight: poll faster so the result appears as
            // soon as it lands, then drop back to idle repainting.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }

        // Apply the theme once per frame.
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = theme::BASE;
        visuals.window_fill = theme::SURFACE;
        visuals.override_text_color = Some(theme::TEXT_PRIMARY);
        ctx.set_visuals(visuals);

        self.draw_sidebar(ctx);
        self.draw_mini_player(ctx);
        self.draw_content(ctx);
        self.draw_toast(ctx);
    }
}

impl GameBridgeApp {
    /// The persistent left sidebar (§20).
    fn draw_sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .exact_width(theme::SIDEBAR_WIDTH)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(theme::SIDEBAR)
                    .inner_margin(theme::SPACE_MD),
            )
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("GAME BRIDGE")
                        .font(theme::card_title_font())
                        .color(theme::TEXT_PRIMARY)
                        .strong(),
                );
                ui.add_space(theme::SPACE_MD);

                for screen in Screen::PRIMARY {
                    self.draw_sidebar_item(ui, screen);
                }

                ui.add_space(theme::SPACE_SM);
                widgets::divider(ui);
                ui.add_space(theme::SPACE_SM);

                for screen in Screen::SECONDARY {
                    self.draw_sidebar_item(ui, screen);
                }

                // Footer: connection status, then the user block (§20).
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(theme::SPACE_SM);
                    widgets::divider(ui);
                    ui.add_space(theme::SPACE_SM);

                    ui.horizontal(|ui| {
                        widgets::dot(ui, theme::status_color(self.status_tone()));
                        ui.label(
                            egui::RichText::new(self.session.state().label())
                                .font(theme::meta_font())
                                .color(theme::TEXT_SECONDARY),
                        );
                    });

                    ui.add_space(theme::SPACE_SM);
                    ui.label(
                        egui::RichText::new("Thanabordee")
                            .font(theme::body_font())
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.label(
                        egui::RichText::new(self.wallet.balance_display())
                            .font(theme::meta_font())
                            .color(theme::ACCENT),
                    );
                });
            });
    }

    fn draw_sidebar_item(&mut self, ui: &mut egui::Ui, screen: Screen) {
        let selected = self.screen == screen;
        let text_color = if selected {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        };
        let fill = if selected {
            theme::ACCENT_WASH
        } else {
            egui::Color32::TRANSPARENT
        };

        let button = egui::Button::new(
            egui::RichText::new(screen.label())
                .font(theme::body_font())
                .color(text_color),
        )
        .fill(fill)
        .rounding(theme::small_radius())
        .min_size(egui::Vec2::new(theme::SIDEBAR_WIDTH - theme::SPACE_MD * 2.0, 34.0));

        if ui.add(button).clicked() {
            self.screen = screen;
        }
        ui.add_space(theme::SPACE_XS);
    }

    /// The persistent translation control bar (§30).
    fn draw_mini_player(&mut self, ctx: &egui::Context) {
        // Only shown while a session exists; §30 calls it persistent, and on an
        // idle Home screen it would be an empty bar.
        if self.session.state().is_stopped() {
            return;
        }

        egui::TopBottomPanel::bottom("mini_player")
            .exact_height(theme::MINI_PLAYER_HEIGHT)
            .frame(
                egui::Frame::none()
                    .fill(theme::SURFACE_RAISED)
                    .inner_margin(egui::Margin::symmetric(theme::SPACE_MD, theme::SPACE_SM)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let tone = self.status_tone();
                    widgets::dot(ui, theme::status_color(tone));
                    ui.label(
                        egui::RichText::new(self.session.state().label().to_uppercase())
                            .font(theme::meta_font())
                            .color(theme::status_color(tone))
                            .strong(),
                    );

                    ui.add_space(theme::SPACE_MD);

                    let application = self
                        .config
                        .session
                        .application
                        .as_deref()
                        .unwrap_or("No game selected");
                    ui.label(
                        egui::RichText::new(application)
                            .font(theme::body_font())
                            .color(theme::TEXT_PRIMARY),
                    );

                    ui.add_space(theme::SPACE_MD);

                    ui.label(
                        egui::RichText::new(self.config.session.pair.short_display())
                            .font(theme::body_font())
                            .color(theme::TEXT_SECONDARY),
                    );

                    ui.add_space(theme::SPACE_MD);

                    // Active-voice clock, not wall-clock: §30 shows the billed
                    // quantity, and showing session time here would misrepresent
                    // what the user is paying for.
                    ui.label(
                        egui::RichText::new(self.session.speech_clock())
                            .font(theme::body_font())
                            .color(theme::TEXT_PRIMARY),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Stop")
                                        .color(theme::TEXT_PRIMARY)
                                        .size(theme::META_SIZE),
                                )
                                .fill(theme::ERROR)
                                .rounding(theme::pill_radius())
                                .min_size(egui::Vec2::new(
                                    72.0,
                                    theme::BUTTON_HEIGHT_SMALL,
                                )),
                            )
                            .clicked()
                        {
                            self.stop_session();
                        }

                        let bypass = self.bypass();
                        let bypass_label = if bypass.active() { "Resume" } else { "Bypass" };
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(bypass_label)
                                        .color(theme::TEXT_PRIMARY)
                                        .size(theme::META_SIZE),
                                )
                                .fill(theme::SURFACE_HOVER)
                                .rounding(theme::pill_radius())
                                .min_size(egui::Vec2::new(
                                    84.0,
                                    theme::BUTTON_HEIGHT_SMALL,
                                )),
                            )
                            .clicked()
                        {
                            self.toggle_bypass();
                        }
                    });
                });
            });
    }

    /// The content area.
    fn draw_content(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(theme::BASE)
                    .inner_margin(theme::SPACE_LG),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    match self.screen {
                        Screen::Home => screens::home::draw(ui, self),
                        Screen::Translation => screens::translation::draw(ui, self),
                        Screen::Voice => screens::voice::draw(ui, self),
                        Screen::Audio => screens::audio::draw(ui, self),
                        Screen::Usage => screens::usage::draw(ui, self),
                        Screen::Billing => screens::billing::draw(ui, self),
                        Screen::Settings => screens::settings::draw(ui, self),
                    }
                });
            });
    }

    /// A transient toast, e.g. after saving settings.
    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some(message) = self.toast.clone() else {
            return;
        };
        egui::TopBottomPanel::top("toast")
            .frame(
                egui::Frame::none()
                    .fill(theme::ACCENT_WASH)
                    .inner_margin(theme::SPACE_SM),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&message)
                            .font(theme::body_font())
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✕").clicked() {
                            self.toast = None;
                        }
                    });
                });
            });
    }
}

/// Whether a routing mode needs the virtual microphone or headphone output.
///
/// Implemented here rather than on the protocol enum because it is a UI-level
/// question about what to warn about, not a property of the wire format.
trait ModeVoiceRequirement {
    /// Whether this mode needs translated voice to have somewhere to go.
    fn needs_virtual_mic_or_voice(self) -> bool;
}

impl ModeVoiceRequirement for RoutingMode {
    fn needs_virtual_mic_or_voice(self) -> bool {
        matches!(self, RoutingMode::VoiceOut | RoutingMode::FullVoice)
    }
}

/// Convenience alias so screens can refer to the app type without a long path.
pub type App = GameBridgeApp;

/// Text modes, re-exported for the overlay settings screen.
pub const TEXT_MODES: [TextMode; 3] = TextMode::ALL;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_is_the_default_screen() {
        assert_eq!(Screen::default(), Screen::Home);
    }

    #[test]
    fn every_screen_appears_exactly_once_in_the_sidebar() {
        // §20 lists four primary items and three after the divider. A screen
        // missing from both groups would be unreachable.
        let mut all: Vec<Screen> = Screen::PRIMARY
            .iter()
            .chain(Screen::SECONDARY.iter())
            .copied()
            .collect();
        let count = all.len();
        all.sort_by_key(|s| s.label());
        all.dedup();
        assert_eq!(all.len(), count, "a screen is listed twice");

        for screen in [
            Screen::Home,
            Screen::Translation,
            Screen::Voice,
            Screen::Audio,
            Screen::Usage,
            Screen::Billing,
            Screen::Settings,
        ] {
            assert!(all.contains(&screen), "{screen:?} is unreachable");
        }
    }

    #[test]
    fn sidebar_groups_match_the_divider_in_the_spec() {
        assert_eq!(Screen::Home.group(), ScreenGroup::Primary);
        assert_eq!(Screen::Audio.group(), ScreenGroup::Primary);
        assert_eq!(Screen::Usage.group(), ScreenGroup::Secondary);
        assert_eq!(Screen::Settings.group(), ScreenGroup::Secondary);
    }

    #[test]
    fn sidebar_labels_match_the_spec() {
        // §20's exact labels.
        assert_eq!(Screen::Home.label(), "Home");
        assert_eq!(Screen::Translation.label(), "Translation");
        assert_eq!(Screen::Voice.label(), "Voice");
        assert_eq!(Screen::Audio.label(), "Audio");
        assert_eq!(Screen::Usage.label(), "Usage");
        assert_eq!(Screen::Billing.label(), "Billing");
        assert_eq!(Screen::Settings.label(), "Settings");
    }

    #[test]
    fn an_idle_app_reports_ready_not_live() {
        let app = GameBridgeApp::default();
        assert_eq!(app.status_tone(), theme::StatusTone::Ready);
        assert!(!app.session.state().is_live());
    }

    #[test]
    fn a_live_session_reports_the_live_tone() {
        let mut app = GameBridgeApp {
            demo_mode: true,
            ..Default::default()
        };
        app.config.session.microphone_id = "{mic}".into();
        // Voice Out needs the virtual microphone to exist, or start_session
        // correctly refuses.
        app.config.session.virtual_mic_id = Some("{virtual}".into());
        app.wallet.apply(10_000, "THB");
        app.start_session();
        assert!(app.session.state().is_live());
        assert_eq!(app.status_tone(), theme::StatusTone::Live);
    }

    #[test]
    fn starting_without_a_microphone_is_blocked_with_an_explanation() {
        let app = GameBridgeApp::default();
        let blocker = app.start_blocker().expect("should be blocked");
        assert!(blocker.contains("microphone"), "{blocker}");
    }

    #[test]
    fn starting_without_the_driver_is_blocked_in_voice_modes() {
        let mut app = GameBridgeApp::default();
        app.config.session.microphone_id = "{mic}".into();
        app.config.session.mode = RoutingMode::VoiceOut;
        app.wallet.apply(10_000, "THB");

        let blocker = app.start_blocker().expect("should be blocked");
        assert!(blocker.contains("Game Bridge Microphone"), "{blocker}");
    }

    #[test]
    fn subtitle_mode_does_not_need_the_driver() {
        // §10 Mode A emits no TTS, so the driver is irrelevant to it. Blocking
        // here would refuse a mode that works.
        let mut app = GameBridgeApp::default();
        app.config.session.microphone_id = "{mic}".into();
        app.config.session.mode = RoutingMode::Subtitle;
        app.wallet.apply(10_000, "THB");
        assert!(app.start_blocker().is_none());
    }

    #[test]
    fn an_empty_balance_blocks_starting() {
        let mut app = GameBridgeApp::default();
        app.config.session.microphone_id = "{mic}".into();
        app.config.session.mode = RoutingMode::Subtitle;
        app.wallet.apply(0, "THB");
        let blocker = app.start_blocker().expect("should be blocked");
        assert!(blocker.contains("credit"), "{blocker}");
    }

    #[test]
    fn stopping_files_a_usage_record() {
        let mut app = GameBridgeApp {
            demo_mode: true,
            ..Default::default()
        };
        app.config.session.microphone_id = "{mic}".into();
        app.config.session.mode = RoutingMode::Subtitle;
        app.config.session.application = Some("Valorant.exe".into());
        app.wallet.apply(10_000, "THB");

        app.start_session();
        app.session.record_speech(60_000);
        app.stop_session();

        assert_eq!(app.usage_history.len(), 1);
        let record = app.usage_history.latest().unwrap();
        assert_eq!(record.application, "Valorant.exe");
        assert_eq!(record.cost_minor, 50, "subtitle tier, one minute");
    }

    #[test]
    fn the_activity_strip_holds_a_bounded_window() {
        // §41: audio buffers are bounded. A UI strip that grew without limit
        // would be a slow leak.
        let mut app = GameBridgeApp::default();
        for i in 0..500 {
            app.push_activity(i as f32 / 500.0);
        }
        assert_eq!(app.activity.len(), ACTIVITY_BARS);
    }

    #[test]
    fn activity_levels_are_clamped() {
        let mut app = GameBridgeApp::default();
        app.push_activity(5.0);
        app.push_activity(-3.0);
        assert!(app.activity.iter().all(|l| (0.0..=1.0).contains(l)));
    }

    #[test]
    fn bypass_toggles_through_the_app() {
        let mut app = GameBridgeApp::default();
        assert!(!app.bypass().active());
        app.toggle_bypass();
        assert!(app.bypass().active());
        app.toggle_bypass();
        assert!(!app.bypass().active());
    }

    #[test]
    fn feed_lines_are_bounded_and_newest_first() {
        let mut app = GameBridgeApp::default();
        let pair = app.config.session.pair;
        for segment in 0..(FEED_CAPACITY + 50) {
            let mut line = FeedLine::new(segment as u64, pair, "19:42");
            line.translation = Some(format!("line {segment}"));
            app.push_feed_line(line);
        }
        assert_eq!(app.feed.len(), FEED_CAPACITY, "the feed must be bounded");

        // Newest first: the most recent segment is at the head.
        let rendered = app.session_feed();
        assert_eq!(rendered.len(), FEED_CAPACITY);
        assert_eq!(rendered[0].segment_id, (FEED_CAPACITY + 49) as u64);
    }

    #[test]
    fn a_feed_line_carries_the_incoming_language_tags() {
        // The user's pair is Thai → English, but a feed line describes incoming
        // audio: teammates speak English and the user reads Thai. Tagging it
        // with the pair's own order would label every line backwards.
        let app = GameBridgeApp::default();
        let line = FeedLine::new(1, app.config.session.pair, "19:42");
        assert_eq!(line.source_tag, "EN", "the original is what teammates said");
        assert_eq!(line.target_tag, "TH", "the translation is what the user reads");
        assert!(!line.has_content(), "a fresh line has no text yet");
    }

    #[test]
    fn clearing_the_feed_empties_it() {
        let mut app = GameBridgeApp::default();
        let pair = app.config.session.pair;
        app.push_feed_line(FeedLine::new(1, pair, "19:42"));
        assert_eq!(app.feed.len(), 1);
        app.clear_feed();
        assert!(app.feed.is_empty());
    }

    // -- incoming translation ------------------------------------------------

    /// An app using the offline demo provider, so the pipeline can be exercised
    /// without a network or a key.
    fn demo_translation_app() -> GameBridgeApp {
        let mut app = GameBridgeApp::default();
        app.config.provider.mode = crate::config::ProviderMode::Demo;
        app.config.session.microphone_id = "{mic}".into();
        app
    }

    /// Poll until `expected` feed lines have a translation, or time out.
    fn wait_for_translations(app: &mut GameBridgeApp, expected: usize) {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.poll_translations();
            let done = app.feed.lines().iter().filter(|l| !l.is_pending()).count();
            if done >= expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out: {done} of {expected} translations arrived"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn submitting_a_transcript_fills_the_feed_with_a_translation() {
        // The whole incoming path: submit → worker → provider → feed.
        let mut app = demo_translation_app();
        // Incoming direction: teammates speak English, the user reads Thai.
        assert!(app.submit_transcript(0, "Push B."));
        wait_for_translations(&mut app, 1);

        let line = app.feed.latest().expect("a feed line");
        assert_eq!(line.original.as_deref(), Some("Push B."));
        assert_eq!(line.translation.as_deref(), Some("ดันบี"));
        assert_eq!(line.source_tag, "EN");
        assert_eq!(line.target_tag, "TH");
    }

    #[test]
    fn the_original_appears_before_the_translation_arrives() {
        // A reader should see the callout immediately rather than an empty
        // panel while the provider is working.
        let mut app = demo_translation_app();
        app.submit_transcript(0, "Push B.");

        let line = app.feed.latest().expect("a feed line");
        assert_eq!(line.original.as_deref(), Some("Push B."));
        assert!(line.is_pending(), "the translation is still in flight");
    }

    #[test]
    fn the_subtitle_model_receives_the_translated_line() {
        // §32: the overlay renders the same model the feed does.
        let mut app = demo_translation_app();
        app.submit_transcript(0, "Push B.");
        wait_for_translations(&mut app, 1);

        let lines = app.subtitle.lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].original.as_deref(), Some("Push B."));
        assert_eq!(lines[0].translation.as_deref(), Some("ดันบี"));
        assert!(lines[0].is_final);
    }

    #[test]
    fn a_translation_updates_its_own_line_rather_than_appending_one() {
        // The doubled-feed bug: treating the transcript and the translation as
        // independent events.
        let mut app = demo_translation_app();
        app.submit_transcript(0, "Push B.");
        wait_for_translations(&mut app, 1);

        assert_eq!(app.feed.len(), 1, "one segment is one line");
    }

    #[test]
    fn several_segments_produce_several_lines() {
        let mut app = demo_translation_app();
        for (id, text) in [(0u64, "Push B."), (1, "Reloading."), (2, "Fall back.")] {
            app.submit_transcript(id, text);
        }
        wait_for_translations(&mut app, 3);

        assert_eq!(app.feed.len(), 3);
        let translations: Vec<&str> = app
            .feed
            .lines()
            .iter()
            .filter_map(|l| l.translation.as_deref())
            .collect();
        assert!(translations.contains(&"ดันบี"), "{translations:?}");
        assert!(translations.contains(&"รีโหลด"), "{translations:?}");
        assert!(translations.contains(&"ถอย"), "{translations:?}");
    }

    #[test]
    fn stats_are_mirrored_from_the_worker() {
        let mut app = demo_translation_app();
        app.submit_transcript(0, "Push B.");
        wait_for_translations(&mut app, 1);

        assert_eq!(app.translation_stats.attempts, 1);
        assert_eq!(app.translation_stats.successes, 1);
        assert!(!app.translation_stats.is_degraded());
    }

    #[test]
    fn an_empty_submission_is_rejected_without_starting_a_worker() {
        let mut app = demo_translation_app();
        assert!(!app.submit_transcript(0, "   "));
        assert!(!app.translator_running());
        assert!(app.feed.is_empty());
    }

    #[test]
    fn submitting_from_the_input_box_consumes_the_text() {
        let mut app = demo_translation_app();
        app.translation_input = "Push B.".to_string();
        assert!(app.submit_translation_input());
        assert!(app.translation_input.is_empty(), "the box is cleared");
        wait_for_translations(&mut app, 1);
        assert_eq!(app.feed.latest().unwrap().translation.as_deref(), Some("ดันบี"));
    }

    #[test]
    fn submitting_an_empty_input_box_does_nothing() {
        let mut app = demo_translation_app();
        app.translation_input = "   ".to_string();
        assert!(!app.submit_translation_input());
        assert!(app.feed.is_empty());
    }

    #[test]
    fn manual_segments_are_numbered_uniquely() {
        let mut app = demo_translation_app();
        for _ in 0..3 {
            app.translation_input = "Push B.".to_string();
            app.submit_translation_input();
        }
        wait_for_translations(&mut app, 3);
        let ids: Vec<u64> = app.feed.lines().iter().map(|l| l.segment_id).collect();
        assert_eq!(ids, vec![0, 1, 2]);
    }

    #[test]
    fn stopping_the_session_stops_the_worker() {
        let mut app = demo_translation_app();
        app.wallet.apply(10_000, "THB");
        app.start_session();
        app.submit_transcript(0, "Push B.");
        wait_for_translations(&mut app, 1);
        assert!(app.translator_running());

        app.stop_session();
        assert!(!app.translator_running(), "the worker should be joined");
    }

    #[test]
    fn the_provider_description_is_exposed_for_the_status_line() {
        let mut app = demo_translation_app();
        app.submit_transcript(0, "Push B.");
        wait_for_translations(&mut app, 1);
        let described = app.translator_description.clone().expect("a description");
        assert!(described.contains("demo"), "{described}");
    }

    #[test]
    fn an_invalid_endpoint_is_reported_without_starting_a_worker() {
        // A remote plaintext endpoint is refused, and the reason must reach the
        // user rather than being swallowed.
        let mut app = GameBridgeApp::default();
        app.config.provider.mode = crate::config::ProviderMode::OpenAi;
        app.config.provider.base_url = "http://remote.example.com/v1".to_string();

        assert!(!app.ensure_worker());
        assert!(!app.translator_running());
        let error = app.translator_error.expect("an error message");
        assert!(error.contains("clear text"), "{error}");
    }

    #[test]
    fn a_failed_translation_is_marked_rather_than_left_looking_pending() {
        // The bug this guards: a failed line rendering as "translating…"
        // forever, which the reader cannot distinguish from a slow provider.
        let mut app = demo_translation_app();
        let unknown = "a phrase the demo dictionary has never heard of";
        app.submit_transcript(0, unknown);
        wait_for_translations(&mut app, 1);

        let line = app.feed.latest().expect("a feed line");
        assert!(line.is_failed());
        assert!(!line.is_pending());
        assert!(line.error.is_some());
        assert_eq!(
            line.original.as_deref(),
            Some(unknown),
            "the original survives a failure"
        );
    }

    #[test]
    fn a_demo_miss_is_reported_as_a_failure_not_a_translation() {
        // A phrase the dictionary does not know must not come back as a
        // success with the input echoed, which would tell the reader it had
        // been translated.
        let mut app = demo_translation_app();
        app.submit_transcript(0, "unmapped phrase");
        wait_for_translations(&mut app, 1);

        assert_eq!(app.translation_stats.successes, 0);
        assert_eq!(app.translation_stats.failures, 1);
        assert!(app.feed.latest().unwrap().translation.is_none());
    }

    #[test]
    fn a_non_actionable_failure_does_not_raise_a_banner() {
        // A dictionary miss is not something the user must fix, so it stays in
        // the feed line and out of their face.
        let mut app = demo_translation_app();
        app.submit_transcript(0, "unmapped phrase");
        wait_for_translations(&mut app, 1);
        assert!(
            app.translator_error.is_none(),
            "unexpected banner: {:?}",
            app.translator_error
        );
    }

    #[test]
    fn a_successful_translation_clears_the_failed_state() {
        let mut app = demo_translation_app();
        app.submit_transcript(0, "Push B.");
        wait_for_translations(&mut app, 1);

        let line = app.feed.latest().unwrap();
        assert!(!line.is_failed());
        assert!(!line.is_pending());
        assert!(line.error.is_none());
    }

    #[test]
    fn submitting_audio_without_a_key_is_reported_rather_than_silent() {
        // The honest failure: no speech-to-text key means no subtitles from
        // audio, and the user must be told why rather than seeing nothing.
        let mut app = demo_translation_app();
        // Demo mode has no transcriber configured.
        app.config.transcription.enabled = false;

        let path = std::env::temp_dir().join("gb-app-audio-test.wav");
        std::fs::write(&path, vec![0u8; 128]).expect("write temp audio");

        assert!(app.submit_audio_file(path.to_string_lossy().as_ref()));
        // Wait for the worker to report back.
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.poll_translations();
            if app.feed.latest().is_some_and(|l| l.is_failed()) {
                break;
            }
            assert!(Instant::now() < deadline, "timed out waiting for a result");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let line = app.feed.latest().expect("a feed line");
        assert!(line.is_failed(), "a missing key must mark the line");
        assert!(
            app.translator_error.is_some(),
            "a missing key needs the user to act"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_audio_file_is_reported_before_a_job_is_queued() {
        let mut app = demo_translation_app();
        assert!(!app.submit_audio_file("/nonexistent/audio.wav"));
        let error = app.translator_error.clone().expect("an error");
        assert!(error.contains("Cannot read"), "{error}");
        assert!(app.feed.is_empty(), "nothing should be queued");
    }

    #[test]
    fn an_empty_audio_path_is_rejected() {
        let mut app = demo_translation_app();
        assert!(!app.submit_audio_file("   "));
        assert!(app.feed.is_empty());
    }

    #[test]
    fn submitting_from_the_audio_box_consumes_the_path() {
        let mut app = demo_translation_app();
        app.audio_path = "/nonexistent/audio.wav".to_string();
        assert!(!app.submit_audio_input(), "the file does not exist");
        assert!(app.audio_path.is_empty(), "the box is cleared either way");
    }

    #[test]
    fn audio_content_types_are_guessed_from_the_extension() {
        assert_eq!(audio_content_type("clip.wav"), "audio/wav");
        assert_eq!(audio_content_type("CLIP.WAV"), "audio/wav");
        assert_eq!(audio_content_type("a.mp3"), "audio/mpeg");
        assert_eq!(audio_content_type("a.ogg"), "audio/ogg");
        assert_eq!(audio_content_type("a.flac"), "audio/flac");
        assert_eq!(audio_content_type("a.m4a"), "audio/mp4");
        // Raw PCM is what the capture path produces.
        assert_eq!(audio_content_type("capture.pcm"), "audio/l16");
        assert_eq!(audio_content_type("noextension"), "audio/l16");
    }

    #[test]
    fn file_labels_strip_the_directory() {
        assert_eq!(file_label("/tmp/a/b/clip.wav"), "clip.wav");
        assert_eq!(file_label("clip.wav"), "clip.wav");
        assert_eq!(file_label(r"C:\Users\me\clip.wav"), "clip.wav");
    }

    /// A result carrying a real transcript, as the worker would produce.
    fn transcribed_result(speech_ms: u64, text: &str) -> IncomingResult {
        use crate::transcribe::Transcription;
        IncomingResult {
            job_id: 0,
            segment_id: 0,
            segment_index: 0,
            audio_ms: speech_ms + 800,
            speech_ms,
            start_ms: 0,
            transcription: Some(Transcription {
                text: text.to_string(),
                confidence: 0.97,
                detected_language: Some(game_bridge_protocol::Language::English),
                language_confidence: Some(0.99),
                audio_duration: None,
                latency: std::time::Duration::from_millis(300),
            }),
            transcription_error: None,
            outcome: None,
            stats: TranslatorStats::default(),
        }
    }

    #[test]
    fn the_active_voice_meter_is_wired_to_the_pipeline() {
        // The gap this closes: the meter existed, was tested, and was never
        // connected to anything. A correct meter that nothing calls reads zero
        // forever, and the user is never charged — or, worse, is charged by a
        // gateway whose numbers the client never agreed with.
        let mut app = demo_translation_app();
        // Rates arrive with the session handshake. Applying them directly keeps
        // this test about the meter rather than about connection setup.
        app.session
            .apply_rates(game_bridge_protocol::usage::CreditRate::launch_defaults());
        assert_eq!(app.metered_speech_ms, 0);

        app.apply_incoming(transcribed_result(60_000, "Push B."), Instant::now());

        assert_eq!(app.metered_speech_ms, 60_000);
        assert_eq!(app.session.usage().active_voice.speech_ms, 60_000);
        assert_eq!(
            app.session.usage().cost_minor, 250,
            "one minute of voice at the launch rate"
        );
    }

    #[test]
    fn the_meter_accumulates_across_utterances() {
        let mut app = demo_translation_app();
        for _ in 0..3 {
            app.apply_incoming(transcribed_result(500, "Push B."), Instant::now());
        }
        assert_eq!(app.metered_speech_ms, 1_500);
        assert_eq!(app.session.usage().active_voice.segments, 3);
    }

    #[test]
    fn nothing_transcribed_meters_nothing() {
        // The music-bed case. The detector said speech; the provider returned
        // nothing; the user must not be charged for their game's soundtrack.
        let mut app = demo_translation_app();
        app.apply_incoming(transcribed_result(2_000, ""), Instant::now());
        assert_eq!(app.metered_speech_ms, 0, "silence must not be billed");
        assert_eq!(app.session.usage().cost_minor, 0);
    }

    #[test]
    fn a_transcription_failure_meters_nothing() {
        use crate::transcribe::TranscribeError;
        let mut app = demo_translation_app();
        let mut result = transcribed_result(2_000, "");
        result.transcription = None;
        result.transcription_error = Some(TranscribeError::Timeout {
            after: std::time::Duration::from_secs(30),
        });

        app.apply_incoming(result, Instant::now());
        assert_eq!(app.metered_speech_ms, 0);
        assert!(app.feed.latest().is_some_and(|line| line.is_failed()));
    }

    #[test]
    fn the_working_indicator_clears_when_results_arrive() {
        let mut app = demo_translation_app();
        app.pending_audio = Some("clip.wav".to_string());
        app.apply_incoming(transcribed_result(600, "Push B."), Instant::now());
        assert!(app.pending_audio.is_none());
    }

    #[test]
    fn polling_without_a_worker_is_harmless() {
        let mut app = GameBridgeApp::default();
        app.poll_translations();
        assert!(app.feed.is_empty());
    }

    #[test]
    fn the_feed_clock_formats_a_known_instant() {
        // Deterministic because the offset is supplied rather than read from
        // the machine.
        let epoch = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let bangkok = time::UtcOffset::from_hms(7, 0, 0).unwrap();
        let formatted = format_clock_at(epoch, bangkok);
        // 1700000000 is 2023-11-14 22:13:20 UTC, so 05:13 in Bangkok.
        assert_eq!(formatted, "05:13");
    }

    #[test]
    fn the_feed_clock_is_always_hh_mm() {
        let formatted = format_clock_local();
        assert_eq!(formatted.len(), 5, "{formatted}");
        assert_eq!(&formatted[2..3], ":");
    }

    #[test]
    fn the_text_mode_list_matches_the_protocol() {
        assert_eq!(TEXT_MODES.len(), 3);
        assert!(TEXT_MODES.contains(&TextMode::Both));
    }
}
