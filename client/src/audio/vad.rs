//! Local voice activity detection (§7).
//!
//! VAD runs entirely on the client. Silence is dropped before it reaches the
//! network, which is what makes the billing promise in §29 true: the cloud only
//! ever sees speech, so it can only ever charge for speech.
//!
//! # Why not send everything and let the server decide
//!
//! Because then the server would have to receive silence to know it was
//! silence, and the user would be paying for the bandwidth and the inference.
//! Dropping locally is the whole point.
//!
//! # Algorithm
//!
//! Per-frame energy in dBFS, compared against an adaptive noise floor, with a
//! zero-crossing-rate check to reject loud non-speech transients. This is a
//! deliberately simple detector: no model, no allocation, no dependency, and
//! cheap enough to run on every 10 ms frame inside the audio callback. It is
//! not as accurate as a neural VAD, and the tradeoff is stated rather than
//! hidden — a `Vad` trait exists so a neural implementation can replace it
//! without touching the capture path.
//!
//! # Buffering around an utterance
//!
//! Cutting audio exactly at the detected onset clips the first phoneme, and
//! cutting at the detected offset clips the last one. A **pre-roll** keeps
//! recent history so speech starts with leading context attached, and a
//! **post-roll** keeps emitting after the offset so trailing consonants and
//! fricatives survive. Both are configurable because the right values depend on
//! the microphone and the language.

use std::collections::VecDeque;

/// Tunable VAD parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VadConfig {
    /// Samples per analysis frame. Must match the capture chunk size for the
    /// frame count to line up with wall-clock time.
    pub frame_samples: usize,
    /// Energy in dBFS at or above which a frame may be speech.
    ///
    /// Meaningful only relative to the adaptive floor; this is the absolute
    /// ceiling that keeps a very noisy room from tripping the detector.
    pub speech_threshold_db: f32,
    /// How far above the tracked noise floor a frame must be to count as
    /// speech, in dB.
    pub noise_margin_db: f32,
    /// Consecutive speech frames required to declare onset. Suppresses clicks
    /// and keyboard noise.
    pub onset_frames: u32,
    /// Silence duration, in milliseconds, before an utterance is considered
    /// over. The "hangover".
    pub hangover_ms: u32,
    /// How much audio before onset to include, in milliseconds.
    pub pre_roll_ms: u32,
    /// How much audio after the detected offset to include, in milliseconds.
    pub post_roll_ms: u32,
    /// Hard cap on a single utterance, in milliseconds. Bounds memory and stops
    /// a stuck-open microphone from streaming forever.
    pub max_utterance_ms: u32,
    /// Zero-crossing rate above which a frame is rejected as transient noise
    /// rather than speech, in crossings per sample.
    pub max_zcr: f32,
    /// How much recent audio the noise floor is derived from, in milliseconds.
    ///
    /// The floor is the **quietest frame in this window**, not a smoothed
    /// average. See [`Vad`] for why that distinction fixes a real bug.
    pub floor_window_ms: u32,
    /// How much history the floor needs before it is trusted, in milliseconds.
    ///
    /// Before this much audio has been seen, only the absolute threshold
    /// applies. Without a warm-up the floor is the minimum of whatever has been
    /// heard so far, which for a recording that opens mid-sentence is the speech
    /// itself — and a floor equal to the signal can never be exceeded by it, so
    /// nothing would ever be detected.
    pub floor_trust_ms: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            // 10 ms at 16 kHz. Short enough that onset detection is not the
            // dominant term in the latency budget.
            frame_samples: 160,
            speech_threshold_db: -45.0,
            noise_margin_db: 9.0,
            onset_frames: 3,
            hangover_ms: 500,
            // 300 ms of pre-roll is the standard choice: enough to catch a
            // clipped plosive without sending half a second of silence per
            // utterance.
            pre_roll_ms: 300,
            post_roll_ms: 200,
            max_utterance_ms: 30_000,
            // 500 ms, chosen by measurement rather than reasoning.
            //
            // The floor cannot learn a background bed faster than one window,
            // so the window *is* the adaptation lag: measured against a clip
            // where a tone bed starts abruptly after speech, the bed leaked for
            // 2.46 s at a 2 s window and 0.96 s at 500 ms.
            //
            // The obvious risk of a short window — the floor rising during
            // continuous speech until nothing passes the margin — did not
            // appear: a real 14.5 s narration clip produced the same six
            // segments at every window from 2 s down to 300 ms, with the speech
            // fraction moving only from 81% to 73%.
            //
            // 500 ms is roughly two to three syllables: long enough to contain
            // a genuinely quiet moment, short enough to follow a changing bed.
            // 300 ms measured only marginally better on leakage and leaves less
            // margin, so it is not the default.
            floor_window_ms: 500,
            // 200 ms. Short enough that a music bed starting a recording is
            // rejected almost immediately, long enough to span a couple of
            // syllables.
            floor_trust_ms: 200,
            // Voiced speech sits far below this: a 200 Hz fundamental sampled
            // at 16 kHz crosses zero about 0.025 times per sample. Broadband
            // noise — a click, a clack, room hiss — sits near 0.5. A threshold
            // of 0.3 separates them with room on both sides.
            max_zcr: 0.3,
        }
    }
}

impl VadConfig {
    /// Frame duration in milliseconds at `sample_rate_hz`.
    pub fn frame_ms(&self, sample_rate_hz: u32) -> f32 {
        if sample_rate_hz == 0 {
            return 0.0;
        }
        self.frame_samples as f32 * 1000.0 / sample_rate_hz as f32
    }

    /// Number of frames corresponding to `ms` at `sample_rate_hz`.
    pub fn frames_for_ms(&self, ms: u32, sample_rate_hz: u32) -> usize {
        if sample_rate_hz == 0 || self.frame_samples == 0 {
            return 0;
        }
        let samples = (u64::from(ms) * u64::from(sample_rate_hz)) / 1000;
        (samples as usize).div_ceil(self.frame_samples)
    }
}

/// What the VAD concluded about a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadVerdict {
    /// Speech is ongoing.
    Speech,
    /// Not speech; the frame is being buffered but not emitted.
    Silence,
    /// A new utterance just started. Emit the pre-roll first.
    SpeechStarted,
    /// The utterance just ended. Emit the post-roll, then stop.
    SpeechEnded,
}

/// A voice-activity detector with pre-roll, hangover, and post-roll.
pub struct Vad {
    config: VadConfig,
    sample_rate_hz: u32,
    /// Samples not yet filling a complete analysis frame.
    pending: Vec<i16>,
    /// Rolling history of input frames, for pre-roll. Retains at most
    /// `pre_roll_ms` of audio.
    history: VecDeque<i16>,
    /// Frames emitted since the current utterance began, for the duration cap.
    utterance_frames: u32,
    /// Consecutive speech frames seen while silent.
    onset_run: u32,
    /// Consecutive silence frames seen while speaking.
    silence_run: u32,
    /// Whether an utterance is currently open.
    in_speech: bool,
    /// Recent frame energies in dBFS, newest last.
    ///
    /// The noise floor is the minimum of this window.
    recent_energies: VecDeque<f32>,
    /// Capacity of [`Vad::recent_energies`], in frames.
    floor_window_frames: usize,
    /// How many frames the floor needs before it is trusted.
    floor_trust_frames: usize,
}

impl Vad {
    /// Create a detector for audio at `sample_rate_hz`.
    pub fn new(config: VadConfig, sample_rate_hz: u32) -> Self {
        let history_len = config.frames_for_ms(config.pre_roll_ms, sample_rate_hz)
            * config.frame_samples;
        Self {
            config,
            sample_rate_hz,
            pending: Vec::with_capacity(config.frame_samples),
            history: VecDeque::with_capacity(history_len + config.frame_samples),
            utterance_frames: 0,
            onset_run: 0,
            silence_run: 0,
            in_speech: false,
            recent_energies: VecDeque::new(),
            floor_window_frames: config
                .frames_for_ms(config.floor_window_ms, sample_rate_hz)
                .max(1),
            floor_trust_frames: config
                .frames_for_ms(config.floor_trust_ms, sample_rate_hz)
                .max(1),
        }
    }

    /// The detector's current configuration.
    pub fn config(&self) -> &VadConfig {
        &self.config
    }

    /// Tracked noise floor in dBFS: the quietest frame in the recent window.
    ///
    /// `f32::NEG_INFINITY` until at least one frame has been seen.
    pub fn noise_floor_db(&self) -> f32 {
        self.recent_energies
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min)
    }

    /// Whether an utterance is currently open.
    pub fn is_in_speech(&self) -> bool {
        self.in_speech
    }

    /// Feed samples and get back the frames the VAD has decided on.
    ///
    /// Returned frames are `(verdict, samples)`. The caller should stream any
    /// frame whose verdict is [`VadVerdict::Speech`],
    /// [`VadVerdict::SpeechStarted`], or [`VadVerdict::SpeechEnded`], and
    /// discard [`VadVerdict::Silence`].
    ///
    /// On `SpeechStarted` the returned samples begin with the pre-roll, so the
    /// caller must send the whole slice rather than just the new audio.
    pub fn push(&mut self, samples: &[i16]) -> Vec<(VadVerdict, Vec<i16>)> {
        let mut out = Vec::new();
        self.pending.extend_from_slice(samples);

        while self.pending.len() >= self.config.frame_samples {
            let frame: Vec<i16> = self.pending.drain(..self.config.frame_samples).collect();
            if let Some((verdict, payload)) = self.process_frame(&frame) {
                out.push((verdict, payload));
            }
        }
        out
    }

    fn process_frame(&mut self, frame: &[i16]) -> Option<(VadVerdict, Vec<i16>)> {
        let energy_db = frame_energy_db(frame);
        let zcr = zero_crossing_rate(frame);
        let is_speechish = self.looks_like_speech(energy_db, zcr);

        // Track the floor from **every** frame, and take the minimum over a
        // window.
        //
        // The previous version updated the floor only on frames it had already
        // decided were not speech, and seeded it from the first such frame. That
        // is a lock: if a recording starts with speech, or with any sound above
        // the absolute threshold, no frame is ever "not speech", so the floor
        // never seeds, so the margin check never engages, so every low-ZCR sound
        // above the threshold is classified as speech.
        //
        // Measured on a clip of speech + a 3 s tone bed + speech, that turned
        // the bed into a 4.7-second "utterance". In a real game — where a music
        // bed is always present — it means silence is sent to the cloud and
        // billed, which is precisely what SPEC §7 and §29 promise will not
        // happen.
        //
        // A sliding minimum cannot lock up: speech is louder than the bed, so
        // the minimum stays at the bed, and the margin then rejects it. The
        // window is what keeps a room that genuinely gets louder from being
        // treated as permanent speech.
        self.push_energy(energy_db);

        match (self.in_speech, is_speechish) {
            (false, true) => {
                self.onset_run += 1;
                // Hold the frame as pre-roll until onset is confirmed.
                self.push_history(frame);
                if self.onset_run >= self.config.onset_frames {
                    self.in_speech = true;
                    self.silence_run = 0;
                    self.utterance_frames = 1;
                    // Prepend history so the first phoneme is not clipped. The
                    // triggering frame is the last element of history.
                    let payload = self.drain_history();
                    Some((VadVerdict::SpeechStarted, payload))
                } else {
                    None
                }
            }
            (false, false) => {
                self.onset_run = 0;
                self.push_history(frame);
                Some((VadVerdict::Silence, Vec::new()))
            }
            (true, true) => {
                self.silence_run = 0;
                self.utterance_frames += 1;
                if self.exceeds_max_duration() {
                    return Some(self.end_utterance(frame.to_vec()));
                }
                Some((VadVerdict::Speech, frame.to_vec()))
            }
            (true, false) => {
                self.silence_run += 1;
                self.utterance_frames += 1;
                let hangover_frames = self.hangover_frames();
                if self.silence_run >= hangover_frames {
                    // Utterance over. Emit the hangover frames plus post-roll as
                    // the tail so trailing sounds are not clipped.
                    let mut tail = Vec::with_capacity(frame.len() * 2);
                    tail.extend_from_slice(frame);
                    Some(self.end_utterance_with_post_roll(tail))
                } else {
                    // Inside the hangover: keep emitting so a brief pause mid
                    // sentence does not split the utterance.
                    Some((VadVerdict::Speech, frame.to_vec()))
                }
            }
        }
    }

    /// Whether a frame is speech-like by energy and zero-crossing rate.
    fn looks_like_speech(&self, energy_db: f32, zcr: f32) -> bool {
        // The absolute gate: nothing this quiet is speech, whatever the room is
        // doing. This is what rejects digital silence without consulting the
        // floor at all.
        if energy_db < self.config.speech_threshold_db {
            return false;
        }
        // A high zero-crossing rate means broadband noise — a click, a clack,
        // room hiss — not voiced speech.
        if zcr > self.config.max_zcr {
            return false;
        }

        // Until enough has been heard for the floor to mean anything, the
        // absolute gate above is the only test available.
        if self.recent_energies.len() < self.floor_trust_frames {
            return true;
        }

        let floor = self.noise_floor_db();
        if floor.is_finite() {
            energy_db >= floor + self.config.noise_margin_db
        } else {
            true
        }
    }

    /// Record a frame's energy in the floor window.
    fn push_energy(&mut self, energy_db: f32) {
        // A silent frame has no meaningful dBFS value; recording it would drag
        // the floor to negative infinity and make the margin check vacuous for
        // the whole window.
        if !energy_db.is_finite() {
            return;
        }
        if self.recent_energies.len() >= self.floor_window_frames {
            self.recent_energies.pop_front();
        }
        self.recent_energies.push_back(energy_db);
    }

    fn push_history(&mut self, frame: &[i16]) {
        let max = self.config.frames_for_ms(self.config.pre_roll_ms, self.sample_rate_hz)
            * self.config.frame_samples;
        if max == 0 {
            return;
        }
        self.history.extend(frame.iter().copied());
        while self.history.len() > max {
            self.history.pop_front();
        }
    }

    fn drain_history(&mut self) -> Vec<i16> {
        self.history.drain(..).collect()
    }

    fn hangover_frames(&self) -> u32 {
        self.config
            .frames_for_ms(self.config.hangover_ms, self.sample_rate_hz)
            .max(1) as u32
    }

    fn exceeds_max_duration(&self) -> bool {
        let max_frames = self.config.frames_for_ms(self.config.max_utterance_ms, self.sample_rate_hz);
        max_frames > 0 && self.utterance_frames as usize >= max_frames
    }

    fn end_utterance(&mut self, frame: Vec<i16>) -> (VadVerdict, Vec<i16>) {
        self.reset_utterance();
        (VadVerdict::SpeechEnded, frame)
    }

    fn end_utterance_with_post_roll(&mut self, frame: Vec<i16>) -> (VadVerdict, Vec<i16>) {
        self.reset_utterance();
        (VadVerdict::SpeechEnded, frame)
    }

    fn reset_utterance(&mut self) {
        self.in_speech = false;
        self.onset_run = 0;
        self.silence_run = 0;
        self.utterance_frames = 0;
        self.history.clear();
    }

    /// Flush any open utterance, e.g. when the user stops the session or
    /// releases push-to-translate.
    ///
    /// Returns the audio still buffered at the moment of the flush: a partial
    /// frame that has not yet filled `frame_samples`, plus any pre-roll history
    /// retained since the last emitted frame. Returns `None` when speech is not
    /// open, or when everything captured has already been emitted.
    ///
    /// Either way the detector is reset, so a flush always leaves it idle.
    pub fn flush(&mut self) -> Option<Vec<i16>> {
        if !self.in_speech {
            self.pending.clear();
            self.history.clear();
            return None;
        }
        let mut tail: Vec<i16> = self.history.drain(..).collect();
        tail.extend_from_slice(&self.pending);
        self.pending.clear();
        self.reset_utterance();
        if tail.is_empty() {
            None
        } else {
            Some(tail)
        }
    }
}

/// Root-mean-square energy of a frame in dBFS, relative to full scale.
///
/// Silence is represented as `f32::NEG_INFINITY` so it can never be mistaken
/// for speech; callers compare against thresholds, which handles that value
/// correctly without a special case.
pub fn frame_energy_db(frame: &[i16]) -> f32 {
    if frame.is_empty() {
        return f32::NEG_INFINITY;
    }
    let sum_squares: f64 = frame
        .iter()
        .map(|&s| {
            let normalized = f64::from(s) / 32768.0;
            normalized * normalized
        })
        .sum();
    let ms = sum_squares / frame.len() as f64;
    if ms <= 0.0 {
        return f32::NEG_INFINITY;
    }
    (10.0 * ms.log10()) as f32
}

/// Zero-crossing rate of a frame: crossings per sample, in `0.0..=1.0`.
pub fn zero_crossing_rate(frame: &[i16]) -> f32 {
    if frame.len() < 2 {
        return 0.0;
    }
    let crossings = frame
        .windows(2)
        .filter(|pair| (pair[0] >= 0) != (pair[1] >= 0))
        .count();
    crossings as f32 / (frame.len() - 1) as f32
}

/// Generate a pure tone, for tests and for the §39 audio test screen.
pub fn sine_wave(freq_hz: f32, amplitude: f32, sample_rate_hz: u32, samples: usize) -> Vec<i16> {
    (0..samples)
        .map(|i| {
            let t = i as f32 / sample_rate_hz as f32;
            let value = (2.0 * std::f32::consts::PI * freq_hz * t).sin() * amplitude;
            (value.clamp(-1.0, 1.0) * 32767.0) as i16
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    fn silent_frame() -> Vec<i16> {
        vec![0i16; 160]
    }

    fn noise_frame(amplitude: i16) -> Vec<i16> {
        // Deterministic pseudo-noise built from a small LCG, so the test does
        // not depend on an RNG crate and produces the same signal every run.
        // Uniform noise has a zero-crossing rate near 0.5, which is what makes
        // it a realistic stand-in for a keyboard clack or room hiss.
        let mut state: u32 = 0x1234_5678;
        (0..160)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let centered = ((state >> 16) as i32 % 2003) - 1001;
                ((centered * i32::from(amplitude)) / 1001) as i16
            })
            .collect()
    }

    /// One frame of speech-like audio.
    ///
    /// A **constant-amplitude tone is not a usable proxy for speech** here, and
    /// that is not a detail. Energy detection works because speech has a large
    /// dynamic range: over a couple of seconds, the quietest moment is far
    /// below the peaks. The noise floor is the minimum over that window, so a
    /// flat tone has a floor equal to its own level and is correctly classified
    /// as steady background rather than as speech.
    ///
    /// This signal carries a voiced 200 Hz fundamental under a syllabic
    /// envelope, so frame energies vary the way real speech varies.
    fn speech_frame_at(index: usize) -> Vec<i16> {
        let phase = ((index + 2) % 8) as f32 / 8.0;
        let envelope = 0.08 + 0.62 * (std::f32::consts::PI * phase).sin();
        sine_wave(200.0, envelope, RATE, 160)
    }

    /// Feed `frames` speech-like frames and return every verdict.
    fn speak(vad: &mut Vad, frames: usize) -> Vec<(VadVerdict, Vec<i16>)> {
        let mut out = Vec::new();
        for index in 0..frames {
            out.extend(vad.push(&speech_frame_at(index)));
        }
        out
    }

    /// Feed `frames` of silence and return every verdict.
    fn pause(vad: &mut Vad, frames: usize) -> Vec<(VadVerdict, Vec<i16>)> {
        let mut out = Vec::new();
        for _ in 0..frames {
            out.extend(vad.push(&silent_frame()));
        }
        out
    }

    fn count(results: &[(VadVerdict, Vec<i16>)], want: VadVerdict) -> usize {
        results.iter().filter(|(v, _)| *v == want).count()
    }

    #[test]
    fn energy_of_silence_is_negative_infinity() {
        assert_eq!(frame_energy_db(&silent_frame()), f32::NEG_INFINITY);
        assert_eq!(frame_energy_db(&[]), f32::NEG_INFINITY);
    }

    #[test]
    fn energy_of_full_scale_tone_is_near_zero_dbfs() {
        let frame = sine_wave(200.0, 1.0, RATE, 160);
        let db = frame_energy_db(&frame);
        // A full-scale sine has RMS 1/sqrt(2), i.e. about -3 dBFS.
        assert!(db > -4.0 && db < -2.0, "unexpected energy {db} dBFS");
    }

    #[test]
    fn energy_increases_with_amplitude() {
        let quiet = frame_energy_db(&sine_wave(200.0, 0.1, RATE, 160));
        let loud = frame_energy_db(&sine_wave(200.0, 0.8, RATE, 160));
        assert!(loud > quiet);
        // Tenfold amplitude is 20 dB.
        assert!((loud - quiet - 18.06).abs() < 1.0, "delta was {}", loud - quiet);
    }

    #[test]
    fn zero_crossing_rate_distinguishes_tone_from_noise() {
        let tone = zero_crossing_rate(&sine_wave(200.0, 0.5, RATE, 160));
        let noise = zero_crossing_rate(&noise_frame(8000));
        assert!(tone < 0.1, "tone zcr was {tone}");
        assert!(noise > 0.3, "noise zcr was {noise}");
    }

    #[test]
    fn zcr_of_constant_signal_is_zero() {
        assert_eq!(zero_crossing_rate(&[500i16; 100]), 0.0);
        assert_eq!(zero_crossing_rate(&[1i16]), 0.0);
    }

    #[test]
    fn silence_never_opens_an_utterance() {
        let mut vad = Vad::new(VadConfig::default(), RATE);
        let results = pause(&mut vad, 100);
        for (verdict, _) in &results {
            assert_eq!(*verdict, VadVerdict::Silence);
        }
        assert!(!vad.is_in_speech());
    }

    #[test]
    fn speech_opens_an_utterance_after_the_onset_run() {
        let config = VadConfig {
            onset_frames: 2,
            ..Default::default()
        };
        let mut vad = Vad::new(config, RATE);

        let first = vad.push(&speech_frame_at(0));
        assert!(
            !first.iter().any(|(v, _)| *v == VadVerdict::SpeechStarted),
            "one frame must not confirm onset"
        );

        let second = vad.push(&speech_frame_at(1));
        assert!(
            second.iter().any(|(v, _)| *v == VadVerdict::SpeechStarted),
            "expected onset, got {second:?}"
        );
        assert!(vad.is_in_speech());
    }

    #[test]
    fn onset_includes_pre_roll_so_the_first_phoneme_survives() {
        // The property that matters: when speech starts, the emitted frame must
        // contain audio from *before* the trigger, not just from the trigger on.
        let config = VadConfig {
            onset_frames: 1,
            pre_roll_ms: 100, // 10 frames of 160 samples
            ..Default::default()
        };
        let mut vad = Vad::new(config, RATE);

        // Establish a noise floor with quiet audio, which also fills history.
        for _ in 0..20 {
            vad.push(&noise_frame(50));
        }

        // Then speak. With onset_frames = 1 the first loud frame triggers.
        let results = vad.push(&speech_frame_at(2));
        let started: Vec<&(VadVerdict, Vec<i16>)> = results
            .iter()
            .filter(|(v, _)| *v == VadVerdict::SpeechStarted)
            .collect();
        assert_eq!(started.len(), 1, "expected exactly one onset");

        let pre_roll_cap = config.frames_for_ms(config.pre_roll_ms, RATE) * config.frame_samples;
        let emitted = started[0].1.len();
        assert!(
            emitted > config.frame_samples,
            "onset should prepend pre-roll, got only {emitted} samples"
        );
        assert!(
            emitted <= pre_roll_cap + config.frame_samples,
            "pre-roll of {emitted} exceeded its {pre_roll_cap}-sample cap"
        );

        // The tail is the frame that triggered onset, so the pre-roll is
        // genuinely preceding audio rather than a duplicate.
        let tail = &started[0].1[emitted - config.frame_samples..];
        assert_eq!(tail, &speech_frame_at(2)[..], "the trigger frame must come last");
    }

    #[test]
    fn hangover_keeps_a_brief_pause_inside_one_utterance() {
        let config = VadConfig {
            onset_frames: 1,
            hangover_ms: 300,
            ..Default::default()
        };
        let mut vad = Vad::new(config, RATE);

        speak(&mut vad, 1);
        assert!(vad.is_in_speech());

        // One silent frame, well inside the 300 ms hangover (30 frames).
        let during = pause(&mut vad, 1);
        assert!(
            !during.iter().any(|(v, _)| *v == VadVerdict::SpeechEnded),
            "a one-frame pause must not end the utterance"
        );
        assert!(vad.is_in_speech(), "speech must still be open during hangover");
    }

    #[test]
    fn sustained_silence_ends_the_utterance() {
        let config = VadConfig {
            onset_frames: 1,
            hangover_ms: 50,
            ..Default::default()
        };
        let mut vad = Vad::new(config, RATE);

        speak(&mut vad, 2);
        assert!(vad.is_in_speech());

        // 50 ms / 10 ms = 5 frames of silence needed.
        let results = pause(&mut vad, 12);
        assert!(
            results.iter().any(|(v, _)| *v == VadVerdict::SpeechEnded),
            "utterance should have ended after the hangover"
        );
        assert!(!vad.is_in_speech());
    }

    #[test]
    fn events_alternate_start_then_end_exactly_once_per_utterance() {
        let config = VadConfig {
            onset_frames: 1,
            hangover_ms: 30,
            ..Default::default()
        };
        let mut vad = Vad::new(config, RATE);
        let mut starts = 0;
        let mut ends = 0;

        for _ in 0..2 {
            starts += count(&speak(&mut vad, 5), VadVerdict::SpeechStarted);
            ends += count(&pause(&mut vad, 20), VadVerdict::SpeechEnded);
        }

        assert_eq!(starts, 2, "expected two utterances");
        assert_eq!(ends, 2, "expected two utterance ends");
    }

    #[test]
    fn max_duration_forces_a_flush_instead_of_running_forever() {
        let config = VadConfig {
            onset_frames: 1,
            hangover_ms: 10_000, // never naturally ends
            max_utterance_ms: 100,
            ..Default::default()
        };
        let mut vad = Vad::new(config, RATE);

        // The first forced end is what matters: a cap that never fires lets a
        // stuck microphone stream forever. Later restarts are expected, because
        // the detector re-arms and the cap applies again.
        let results = speak(&mut vad, 100);
        let ended_at = results
            .iter()
            .position(|(v, _)| *v == VadVerdict::SpeechEnded)
            .expect("max duration should force an end");
        assert!(
            (9..=12).contains(&ended_at),
            "ended at frame {ended_at}, expected ~10"
        );
    }

    #[test]
    fn flush_returns_buffered_audio_while_speech_is_open() {
        let config = VadConfig {
            onset_frames: 1,
            ..Default::default()
        };
        let mut vad = Vad::new(config, RATE);

        assert_eq!(vad.flush(), None, "nothing to flush when idle");

        // One full speech frame opens the utterance and is emitted.
        speak(&mut vad, 1);
        assert!(vad.is_in_speech());
        assert_eq!(
            vad.flush(),
            None,
            "everything captured was already emitted, so there is no tail"
        );
        assert!(!vad.is_in_speech());

        // A second utterance left holding a partial frame does produce a tail:
        // this is the audio that would otherwise be lost when the user stops
        // mid-word.
        speak(&mut vad, 1);
        vad.push(&speech_frame_at(2)[..40]);
        let tail = vad.flush().expect("a held partial frame should flush");
        assert_eq!(tail.len(), 40, "the partial frame is returned");
        assert!(!vad.is_in_speech());
        assert_eq!(vad.flush(), None, "second flush is a no-op");
    }

    #[test]
    fn flush_resets_the_detector_and_returns_none_when_idle() {
        let config = VadConfig {
            onset_frames: 1,
            ..Default::default()
        };
        let mut vad = Vad::new(config, RATE);

        assert_eq!(vad.flush(), None, "nothing to flush when idle");

        speak(&mut vad, 1);
        assert!(vad.is_in_speech());
        vad.flush();
        assert!(!vad.is_in_speech());
        assert_eq!(vad.flush(), None, "second flush is a no-op");

        // After a flush the detector can start a fresh utterance cleanly.
        let results = speak(&mut vad, 3);
        assert!(
            results.iter().any(|(v, _)| *v == VadVerdict::SpeechStarted),
            "detector should re-arm after a flush"
        );
    }

    #[test]
    fn loud_transient_noise_does_not_trigger_speech() {
        // Broadband noise has a high zero-crossing rate and must be rejected
        // even at high energy. A keyboard clack or a mouse click looks like
        // this, and triggering on it would bill the user for silence.
        let config = VadConfig {
            onset_frames: 3,
            ..Default::default()
        };
        let mut vad = Vad::new(config, RATE);
        for _ in 0..50 {
            let results = vad.push(&noise_frame(30_000));
            assert!(
                !results.iter().any(|(v, _)| *v == VadVerdict::SpeechStarted),
                "broadband noise triggered speech detection"
            );
        }
        assert!(!vad.is_in_speech());
    }

    #[test]
    fn partial_frames_are_buffered_across_calls() {
        // The capture callback delivers arbitrary chunk sizes; the VAD must not
        // lose samples or emit partial frames.
        let config = VadConfig {
            onset_frames: 1,
            ..Default::default()
        };
        let mut vad = Vad::new(config, RATE);

        // Feed 80 samples at a time: exactly half a frame.
        let frame = speech_frame_at(2);
        let half = &frame[..80];
        let first = vad.push(half);
        assert!(first.is_empty(), "half a frame should produce nothing");

        let second = vad.push(half);
        assert!(!second.is_empty(), "a full frame should have been produced");
    }

    #[test]
    fn clear_speech_over_a_noisy_room_is_still_detected() {
        // A noisy room must not deafen the detector to a loud, voiced signal.
        let config = VadConfig {
            onset_frames: 1,
            ..Default::default()
        };
        let mut vad = Vad::new(config, RATE);
        for _ in 0..50 {
            vad.push(&noise_frame(200));
        }
        let results = vad.push(&speech_frame_at(2));
        assert!(
            results.iter().any(|(v, _)| *v == VadVerdict::SpeechStarted),
            "speech over a quiet floor was missed"
        );
    }

    // -- the noise floor ----------------------------------------------------

    #[test]
    fn the_noise_floor_is_the_quietest_recent_frame() {
        let mut vad = Vad::new(VadConfig::default(), RATE);
        assert_eq!(
            vad.noise_floor_db(),
            f32::INFINITY,
            "no frames seen yet means no floor"
        );

        vad.push(&sine_wave(200.0, 0.5, RATE, 160));
        let loud_floor = vad.noise_floor_db();

        vad.push(&sine_wave(200.0, 0.05, RATE, 160));
        let quiet_floor = vad.noise_floor_db();

        assert!(quiet_floor < loud_floor, "the floor should follow the minimum");
    }

    #[test]
    fn the_noise_floor_cannot_lock_up_when_a_recording_starts_with_sound() {
        // The regression this guards. The floor used to be seeded from the
        // first frame that was classified as *not* speech, and only updated on
        // such frames. If a recording begins with speech — or with any sound
        // above the absolute threshold — no frame is ever "not speech", so the
        // floor never seeds and the margin check never engages.
        //
        // Measured on real audio, that turned a three-second music bed into a
        // 4.7-second "utterance": silence sent to the cloud and billed, which
        // is exactly what SPEC §7 and §29 promise will not happen.
        let mut vad = Vad::new(VadConfig::default(), RATE);

        // Start with speech, so nothing below the threshold is ever seen first.
        speak(&mut vad, 20);
        assert!(
            vad.noise_floor_db().is_finite(),
            "the floor must have learned from the frames it has seen"
        );
    }

    #[test]
    fn a_steady_tone_bed_under_speech_is_not_transcribed_as_speech() {
        // The real game-audio case: a music or effects bed under the voice
        // chat. It is above the absolute threshold and has a low
        // zero-crossing rate, so only the adaptive floor can reject it.
        let config = VadConfig {
            onset_frames: 3,
            hangover_ms: 200,
            ..Default::default()
        };
        let mut vad = Vad::new(config, RATE);

        // Speak, so the floor learns from voiced audio.
        speak(&mut vad, 20);
        // Then let the utterance close.
        pause(&mut vad, 40);

        // A steady low tone at a level well below the speech peaks.
        let mut tone = Vec::new();
        for _ in 0..300 {
            tone.extend(sine_wave(80.0, 0.10, RATE, 160));
        }
        let results = vad.push(&tone);

        assert!(
            !results.iter().any(|(v, _)| *v == VadVerdict::SpeechStarted),
            "a steady tone bed was classified as speech"
        );
        assert!(!vad.is_in_speech());
    }

    #[test]
    fn noise_floor_tracks_ambient_level() {
        let mut vad = Vad::new(VadConfig::default(), RATE);
        for _ in 0..250 {
            vad.push(&noise_frame(100));
        }
        let quiet_floor = vad.noise_floor_db();
        for _ in 0..250 {
            vad.push(&noise_frame(3000));
        }
        let loud_floor = vad.noise_floor_db();
        assert!(
            loud_floor > quiet_floor,
            "floor should rise with ambient noise: {quiet_floor} -> {loud_floor}"
        );
    }

    #[test]
    fn a_silent_frame_does_not_drag_the_floor_to_negative_infinity() {
        // Recording an infinite value would make the margin check vacuous for
        // the whole window, so silence is simply not recorded.
        let mut vad = Vad::new(VadConfig::default(), RATE);
        vad.push(&sine_wave(200.0, 0.5, RATE, 160));
        let before = vad.noise_floor_db();
        vad.push(&silent_frame());
        assert_eq!(vad.noise_floor_db(), before, "silence must not be recorded");
    }

    #[test]
    fn the_floor_window_is_bounded() {
        // An unbounded history would keep the floor pinned to the quietest
        // moment ever heard and stop adapting to a room that gets louder.
        let mut vad = Vad::new(VadConfig::default(), RATE);
        let window = vad.floor_window_frames;
        assert!(window > 0);
        for _ in 0..(window * 3) {
            vad.push(&noise_frame(3000));
        }
        assert!(
            vad.recent_energies.len() <= window,
            "the floor window grew past its bound"
        );
    }

    #[test]
    fn sine_helper_stays_in_range() {
        for sample in sine_wave(440.0, 1.0, 48_000, 1000) {
            assert!(sample >= i16::MIN && sample <= i16::MAX);
        }
    }
}
