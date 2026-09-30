//! The complete incoming pipeline: audio → text → translation → subtitle.
//!
//! [`translate::worker`](crate::translate::worker) runs translation off the UI
//! thread. This module runs the **whole chain** off it, because speech-to-text
//! is also a blocking network call and chaining two blocking calls in the UI
//! loop would freeze the window for the sum of both.
//!
//! ```text
//! UI thread                          worker thread
//! ─────────                          ─────────────
//! submit_audio(id, bytes) ────────►  transcribe  (blocking)
//!                                    translate   (blocking)
//! drain() ◄────────────────────────  IncomingResult
//! ```
//!
//! One thread handles both stages rather than two pipelined threads. That is a
//! deliberate simplification: utterances are short and arrive one at a time, so
//! pipelining would buy nothing and would cost a second set of shutdown and
//! back-pressure problems. If utterances ever overlap, the seam to change is
//! this one module.
//!
//! # Text still works without a transcriber
//!
//! The transcriber is optional. With no speech-to-text configured, text jobs
//! still flow — which is what lets the incoming path be developed and tested
//! before audio capture exists.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use crate::transcribe::{
    is_translatable, AudioInput, TranscribeError, Transcription, TranscriptionProvider,
};
use crate::translate::{
    IncomingTranslator, TranslationOutcome, TranslatorStats, TranslationWorker,
};
use crate::LanguagePair;

/// Work submitted to the pipeline.
enum Job {
    /// Audio to transcribe and then translate.
    Audio {
        segment_id: u64,
        data: Vec<u8>,
        content_type: String,
    },
    /// A transcript to translate directly.
    Text { segment_id: u64, text: String },
}

/// The result of one job.
///
/// Carries whichever stages actually ran. A job that failed at transcription
/// has a `transcription_error` and no `outcome`; one that transcribed to
/// nothing translatable has a `transcription` and no `outcome`, and that is a
/// success, not a failure.
#[derive(Debug, Clone, PartialEq)]
pub struct IncomingResult {
    /// The job this came from.
    ///
    /// An audio job produces one result per utterance the VAD found, so the
    /// job id is what ties them together for the UI.
    pub job_id: u64,
    /// The segment this came from. Unique across the session.
    pub segment_id: u64,
    /// Index of this utterance within its job.
    pub segment_index: u32,
    /// Length of the audio sent to the provider, in milliseconds.
    pub audio_ms: u64,
    /// Voiced duration, excluding the VAD's pre-roll and hangover padding.
    ///
    /// **This is the quantity the meter counts**, and only when the provider
    /// actually returned speech. See the module docs.
    pub speech_ms: u64,
    /// Where the utterance sits in its source, in milliseconds.
    pub start_ms: u64,
    /// The transcription, when the job involved audio and it succeeded.
    pub transcription: Option<Transcription>,
    /// Why transcription failed, when it did.
    pub transcription_error: Option<TranscribeError>,
    /// The translation, when the text was worth translating.
    pub outcome: Option<TranslationOutcome>,
    /// Running translation counters after this job.
    pub stats: TranslatorStats,
}

impl IncomingResult {
    /// Whether a translation was produced.
    pub fn is_translated(&self) -> bool {
        self.outcome.as_ref().is_some_and(|outcome| outcome.is_success())
    }

    /// Whether anything went wrong.
    pub fn is_error(&self) -> bool {
        self.transcription_error.is_some()
            || self
                .outcome
                .as_ref()
                .is_some_and(|outcome| outcome.is_failure())
    }

    /// Whether transcription produced nothing worth translating.
    ///
    /// Silence and noise are the normal case in game audio, so this is not an
    /// error and must not be reported as one.
    pub fn is_skipped(&self) -> bool {
        self.transcription_error.is_none()
            && self.outcome.is_none()
            && self.transcription.is_some()
    }

    /// The text a reader should see, if any.
    pub fn display_text(&self) -> Option<&str> {
        if let Some(outcome) = &self.outcome {
            return Some(outcome.display_text());
        }
        self.transcription.as_ref().map(|t| t.text.as_str())
    }

    /// The original text, if there is any.
    pub fn original(&self) -> Option<&str> {
        if let Some(outcome) = &self.outcome {
            return Some(outcome.original.as_str());
        }
        self.transcription.as_ref().map(|t| t.text.as_str())
    }

    /// A user-facing error message, when there is one worth showing.
    pub fn error_message(&self) -> Option<String> {
        if let Some(error) = &self.transcription_error {
            return Some(error.user_message());
        }
        self.outcome
            .as_ref()
            .and_then(|outcome| outcome.error_message())
    }

    /// Milliseconds to add to the active-voice meter for this result.
    ///
    /// Zero unless the provider returned speech. This is the whole reason the
    /// VAD is not trusted to bill on its own: measured against a real film clip,
    /// it classified the music intro as speech, and speech-to-text correctly
    /// returned nothing for it. Billing follows the transcript — what was
    /// actually transcribed — not the detector's opinion.
    pub fn billable_ms(&self) -> u64 {
        let produced_speech = self
            .transcription
            .as_ref()
            .is_some_and(|transcription| is_translatable(transcription));
        if produced_speech {
            self.speech_ms
        } else {
            0
        }
    }

    /// Whether the failure needs the user to act.
    pub fn needs_user_action(&self) -> bool {
        self.transcription_error
            .as_ref()
            .is_some_and(|error| error.needs_user_action())
            || self
                .outcome
                .as_ref()
                .is_some_and(|outcome| outcome.needs_user_action())
    }
}

/// Runs the incoming chain on a dedicated thread.
pub struct IncomingWorker {
    jobs: Sender<Job>,
    results: Receiver<IncomingResult>,
    handle: Option<JoinHandle<()>>,
    /// Whether a transcriber is configured, for the UI to report.
    has_transcriber: bool,
}

impl IncomingWorker {
    /// Start a worker.
    ///
    /// `transcriber` may be `None`, in which case audio jobs fail with a
    /// missing-key error and text jobs work normally.
    pub fn spawn(
        transcriber: Option<Arc<dyn TranscriptionProvider>>,
        translator: IncomingTranslator,
    ) -> Self {
        let has_transcriber = transcriber.is_some();
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (result_tx, result_rx) = mpsc::channel::<IncomingResult>();

        let handle = thread::Builder::new()
            .name("game-bridge-incoming".to_string())
            .spawn(move || worker_loop(transcriber, translator, job_rx, result_tx))
            .expect("spawning the incoming worker should not fail");

        Self {
            jobs: job_tx,
            results: result_rx,
            handle: Some(handle),
            has_transcriber,
        }
    }

    /// Whether speech-to-text is configured.
    pub fn has_transcriber(&self) -> bool {
        self.has_transcriber
    }

    /// Queue audio to transcribe and translate.
    pub fn submit_audio(
        &self,
        segment_id: u64,
        data: Vec<u8>,
        content_type: impl Into<String>,
    ) -> bool {
        self.jobs
            .send(Job::Audio {
                segment_id,
                data,
                content_type: content_type.into(),
            })
            .is_ok()
    }

    /// Queue a transcript to translate.
    pub fn submit_text(&self, segment_id: u64, text: impl Into<String>) -> bool {
        self.jobs
            .send(Job::Text {
                segment_id,
                text: text.into(),
            })
            .is_ok()
    }

    /// Take the next completed result, if there is one.
    pub fn try_recv(&self) -> Option<IncomingResult> {
        match self.results.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }

    /// Take every completed result currently available.
    pub fn drain(&self) -> Vec<IncomingResult> {
        let mut out = Vec::new();
        while let Some(result) = self.try_recv() {
            out.push(result);
        }
        out
    }

    /// Whether the worker thread is still running.
    pub fn is_alive(&self) -> bool {
        self.handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }

    /// Stop the worker and wait for its thread.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let (dead_tx, _dead_rx) = mpsc::channel::<Job>();
        self.jobs = dead_tx;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for IncomingWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl std::fmt::Debug for IncomingWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncomingWorker")
            .field("alive", &self.is_alive())
            .field("has_transcriber", &self.has_transcriber)
            .finish()
    }
}

/// The worker thread body.
fn worker_loop(
    transcriber: Option<Arc<dyn TranscriptionProvider>>,
    mut translator: IncomingTranslator,
    jobs: Receiver<Job>,
    results: Sender<IncomingResult>,
) {
    let mut next_segment_id = 0u64;

    while let Ok(job) = jobs.recv() {
        let produced = match job {
            Job::Text { segment_id, text } => {
                let outcome = translator.translate(segment_id, &text);
                vec![IncomingResult {
                    job_id: segment_id,
                    segment_id,
                    segment_index: 0,
                    audio_ms: 0,
                    speech_ms: 0,
                    start_ms: 0,
                    transcription: None,
                    transcription_error: None,
                    outcome: Some(outcome),
                    stats: translator.stats(),
                }]
            }
            Job::Audio {
                segment_id,
                data,
                content_type,
            } => run_audio_job(
                transcriber.as_deref(),
                &mut translator,
                &mut next_segment_id,
                segment_id,
                &data,
                &content_type,
            ),
        };

        for result in produced {
            if results.send(result).is_err() {
                return;
            }
        }
    }
}

/// Transcribe then translate one piece of audio, producing one result per
/// utterance the VAD found.
fn run_audio_job(
    transcriber: Option<&dyn TranscriptionProvider>,
    translator: &mut IncomingTranslator,
    next_segment_id: &mut u64,
    job_id: u64,
    data: &[u8],
    content_type: &str,
) -> Vec<IncomingResult> {
    let Some(transcriber) = transcriber else {
        return vec![IncomingResult {
            job_id,
            segment_id: take_id(next_segment_id),
            segment_index: 0,
            audio_ms: 0,
            speech_ms: 0,
            start_ms: 0,
            transcription: None,
            transcription_error: Some(TranscribeError::MissingKey {
                env_var: crate::transcribe::DEFAULT_DEEPGRAM_KEY_ENV.to_string(),
            }),
            outcome: None,
            stats: translator.stats(),
        }];
    };

    // Decode and segment locally when the container is one this client can
    // read. Segmenting is what makes the billing honest: each utterance is
    // transcribed and billed on its own, instead of a whole recording being
    // charged as if it were all speech.
    let utterances = match crate::audio::wav::decode(data) {
        Ok(audio) => {
            let pcm = crate::audio::segmenter::to_wire_pcm(&audio);
            crate::audio::segmenter::segment(
                &pcm,
                crate::audio::vad::VadConfig::default(),
                crate::audio::segmenter::WIRE_RATE,
            )
        }
        // A container this client cannot decode — compressed audio, or raw PCM
        // with no header. Fall back to sending the payload whole rather than
        // refusing it: the provider can read formats the client cannot.
        Err(_) => vec![crate::audio::segmenter::Segment {
            index: 0,
            samples: Vec::new(),
            start_ms: 0,
            audio_ms: 0,
            speech_ms: 0,
        }],
    };

    if utterances.is_empty() {
        // The VAD found nothing. That is the normal case in game audio, not a
        // failure, and it must not bill anything.
        return vec![IncomingResult {
            job_id,
            segment_id: take_id(next_segment_id),
            segment_index: 0,
            audio_ms: 0,
            speech_ms: 0,
            start_ms: 0,
            transcription: Some(crate::transcribe::Transcription {
                text: String::new(),
                confidence: 0.0,
                detected_language: None,
                language_confidence: None,
                audio_duration: None,
                latency: std::time::Duration::ZERO,
            }),
            transcription_error: None,
            outcome: None,
            stats: translator.stats(),
        }];
    }

    let mut results = Vec::with_capacity(utterances.len());

    for utterance in &utterances {
        let segment_id = take_id(next_segment_id);

        // Each utterance goes to the provider as a self-describing WAV, so the
        // adapter does not need to know how the audio was produced.
        let payload;
        let (bytes, mime): (&[u8], &str) = if utterance.samples.is_empty() {
            (data, content_type)
        } else {
            payload = encode_wav(&utterance.samples, crate::audio::segmenter::WIRE_RATE);
            (&payload, "audio/wav")
        };

        let input = AudioInput::Bytes {
            data: bytes,
            content_type: mime,
        };

        let transcription = match transcriber.transcribe(&input) {
            Ok(transcription) => transcription,
            Err(error) => {
                results.push(IncomingResult {
                    job_id,
                    segment_id,
                    segment_index: utterance.index,
                    audio_ms: utterance.audio_ms,
                    speech_ms: utterance.speech_ms,
                    start_ms: utterance.start_ms,
                    transcription: None,
                    transcription_error: Some(error),
                    outcome: None,
                    stats: translator.stats(),
                });
                continue;
            }
        };

        // Silence and noise are the normal case in game audio. Not translating
        // them is correct, not a failure, and the result carries the
        // transcription with no outcome.
        if !is_translatable(&transcription) {
            results.push(IncomingResult {
                job_id,
                segment_id,
                segment_index: utterance.index,
                audio_ms: utterance.audio_ms,
                speech_ms: utterance.speech_ms,
                start_ms: utterance.start_ms,
                transcription: Some(transcription),
                transcription_error: None,
                outcome: None,
                stats: translator.stats(),
            });
            continue;
        }

        // Translate from the language that was actually detected. This is the
        // point of multilingual transcription: a teammate speaking a language
        // nobody expected is translated from the right one, rather than through
        // a request that assumes otherwise.
        let text = transcription.text.clone();
        let mut outcome = translator.translate(segment_id, &text);

        if let Some(detected) = transcription.detected_language {
            if detected != translator.pair().target {
                let corrected = LanguagePair::new(translator.pair().source, detected);
                translator.set_pair(corrected);
                outcome = translator.translate(segment_id, &text);
            }
        }

        results.push(IncomingResult {
            job_id,
            segment_id,
            segment_index: utterance.index,
            audio_ms: utterance.audio_ms,
            speech_ms: utterance.speech_ms,
            start_ms: utterance.start_ms,
            transcription: Some(transcription),
            transcription_error: None,
            outcome: Some(outcome),
            stats: translator.stats(),
        });
    }

    results
}

/// Allocate the next session-unique segment id.
fn take_id(next: &mut u64) -> u64 {
    let id = *next;
    *next += 1;
    id
}

/// Encode mono 16-bit PCM as a WAV payload.
fn encode_wav(samples: &[i16], sample_rate_hz: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);

    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate_hz.to_le_bytes());
    out.extend_from_slice(&(sample_rate_hz * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// Convenience: build the text-only worker this module supersedes.
///
/// Kept so callers that only want translation can keep using
/// [`TranslationWorker`] without constructing a transcriber.
pub fn text_only(translator: IncomingTranslator) -> TranslationWorker {
    TranslationWorker::spawn(translator)
}

/// Builders shared with the UI tests.
///
/// The UI needs a real WAV containing real speech to exercise the meter, and
/// building one is fiddly enough that duplicating it would guarantee the two
/// copies drift.
#[cfg(any(test, feature = "gui"))]
pub mod tests_support {
    use crate::audio::vad::sine_wave;
    use crate::audio::segmenter::WIRE_RATE;

    /// Mono 16-bit PCM as a WAV payload.
    pub fn wav(samples: &[i16], sample_rate_hz: u32) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut out = Vec::with_capacity(44 + samples.len() * 2);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&sample_rate_hz.to_le_bytes());
        out.extend_from_slice(&(sample_rate_hz * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        out
    }

    /// A WAV containing `utterances` runs of speech-like audio.
    ///
    /// Speech-like, not a tone: a steady signal has a noise floor equal to
    /// itself and is correctly classified as background.
    pub fn speech_wav(utterances: usize) -> Vec<u8> {
        let mut samples: Vec<i16> = Vec::new();
        for _ in 0..utterances {
            for index in 0..30 {
                let phase = ((index + 2) % 8) as f32 / 8.0;
                let envelope = 0.08 + 0.62 * (std::f32::consts::PI * phase).sin();
                samples.extend(sine_wave(200.0, envelope, WIRE_RATE, 160));
            }
            samples.extend(std::iter::repeat(0i16).take(80 * 160));
        }
        wav(&samples, WIRE_RATE)
    }

    /// A WAV of digital silence.
    pub fn silent_wav(seconds: u32) -> Vec<u8> {
        let samples = vec![0i16; (seconds as usize) * (WIRE_RATE as usize)];
        wav(&samples, WIRE_RATE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcribe::TranscriptionLanguage;
    use crate::translate::mock::ScriptedTranslator;
    use crate::translate::TranslateError;
    use game_bridge_protocol::Language;
    use std::time::{Duration, Instant};

    /// A transcriber that returns a fixed transcript, for driving the chain.
    struct FixedTranscriber {
        result: Result<Transcription, TranscribeError>,
    }

    impl FixedTranscriber {
        fn saying(text: &str, language: Option<Language>, confidence: f32) -> Self {
            Self {
                result: Ok(Transcription {
                    text: text.to_string(),
                    confidence,
                    detected_language: language,
                    language_confidence: Some(0.99),
                    audio_duration: Some(Duration::from_secs(3)),
                    latency: Duration::from_millis(350),
                }),
            }
        }

        fn failing(error: TranscribeError) -> Self {
            Self { result: Err(error) }
        }
    }

    impl TranscriptionProvider for FixedTranscriber {
        fn transcribe(&self, _audio: &AudioInput<'_>) -> Result<Transcription, TranscribeError> {
            self.result.clone()
        }
        fn describe(&self) -> String {
            "fixed (tests)".to_string()
        }
    }

    fn pair() -> LanguagePair {
        // The user speaks Thai; teammates are assumed to speak English.
        LanguagePair::new(Language::Thai, Language::English)
    }

    fn worker_with(transcriber: Option<Arc<dyn TranscriptionProvider>>) -> IncomingWorker {
        let provider = Arc::new(ScriptedTranslator::always("translated"));
        IncomingWorker::spawn(transcriber, IncomingTranslator::new(provider, pair()))
    }

    fn collect(worker: &IncomingWorker, count: usize) -> Vec<IncomingResult> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut out = Vec::new();
        while out.len() < count {
            out.extend(worker.drain());
            if out.len() >= count {
                break;
            }
            assert!(Instant::now() < deadline, "timed out with {} of {count}", out.len());
            thread::sleep(Duration::from_millis(5));
        }
        out
    }

    // -- billing ------------------------------------------------------------

    /// Build a WAV containing speech-like audio, so the local VAD finds an
    /// utterance in it.
    fn speech_wav(utterances: usize) -> Vec<u8> {
        use crate::audio::vad::sine_wave;
        const RATE: u32 = crate::audio::segmenter::WIRE_RATE;

        let mut samples: Vec<i16> = Vec::new();
        for _ in 0..utterances {
            // A voiced carrier under a syllabic envelope. A constant tone would
            // be classified as background, because its noise floor equals
            // itself.
            for index in 0..30 {
                let phase = ((index + 2) % 8) as f32 / 8.0;
                let envelope = 0.08 + 0.62 * (std::f32::consts::PI * phase).sin();
                samples.extend(sine_wave(200.0, envelope, RATE, 160));
            }
            // A gap long enough to close the utterance.
            samples.extend(std::iter::repeat(0i16).take(80 * 160));
        }
        encode_wav(&samples, RATE)
    }

    #[test]
    fn nothing_transcribed_bills_nothing() {
        // The music-bed case, and the whole reason the meter does not trust the
        // VAD on its own: measured against a real film clip, the detector called
        // the music intro speech and speech-to-text correctly returned nothing.
        let transcriber = Arc::new(FixedTranscriber::saying("", Some(Language::English), 0.0));
        let worker = worker_with(Some(transcriber));
        worker.submit_audio(0, speech_wav(1), "audio/wav");

        let results = collect(&worker, 1);
        assert_eq!(
            results[0].billable_ms(),
            0,
            "audio the provider transcribed to nothing must not be billed"
        );
    }

    #[test]
    fn a_transcribed_utterance_bills_its_speech_duration() {
        let transcriber = Arc::new(FixedTranscriber::saying(
            "Push B.",
            Some(Language::English),
            0.97,
        ));
        let worker = worker_with(Some(transcriber));
        worker.submit_audio(0, speech_wav(1), "audio/wav");

        let results = collect(&worker, 1);
        let result = &results[0];
        assert!(result.is_translated());
        assert!(
            result.billable_ms() > 0,
            "a transcribed utterance must be billed"
        );
        assert_eq!(
            result.billable_ms(),
            result.speech_ms,
            "the billed quantity is the voiced duration"
        );
    }

    #[test]
    fn billing_excludes_the_vad_padding() {
        // Pre-roll and hangover exist to avoid clipping words; they are
        // silence, and charging for them would quietly inflate every utterance.
        let transcriber = Arc::new(FixedTranscriber::saying(
            "Push B.",
            Some(Language::English),
            0.97,
        ));
        let worker = worker_with(Some(transcriber));
        worker.submit_audio(0, speech_wav(1), "audio/wav");

        let results = collect(&worker, 1);
        let result = &results[0];
        assert!(
            result.speech_ms < result.audio_ms,
            "speech {} ms should be under the {} ms sent",
            result.speech_ms,
            result.audio_ms
        );
    }

    #[test]
    fn an_audio_file_with_two_utterances_produces_two_results() {
        // Segmenting is what makes billing per-utterance possible, and it also
        // means a long recording appears as several subtitle lines rather than
        // one wall of text.
        let transcriber = Arc::new(FixedTranscriber::saying(
            "Push B.",
            Some(Language::English),
            0.97,
        ));
        let worker = worker_with(Some(transcriber));
        worker.submit_audio(7, speech_wav(2), "audio/wav");

        let results = collect(&worker, 2);
        assert_eq!(results.len(), 2, "one result per utterance");
        assert_eq!(results[0].job_id, 7);
        assert_eq!(results[1].job_id, 7);
        assert_eq!(results[0].segment_index, 0);
        assert_eq!(results[1].segment_index, 1);
        assert_ne!(
            results[0].segment_id, results[1].segment_id,
            "each utterance needs its own id"
        );
    }

    #[test]
    fn segment_ids_are_unique_across_jobs() {
        let transcriber = Arc::new(FixedTranscriber::saying(
            "Push B.",
            Some(Language::English),
            0.97,
        ));
        let worker = worker_with(Some(transcriber));
        worker.submit_audio(0, speech_wav(1), "audio/wav");
        worker.submit_audio(1, speech_wav(1), "audio/wav");

        let results = collect(&worker, 2);
        assert_ne!(
            results[0].segment_id, results[1].segment_id,
            "ids must not collide between jobs"
        );
    }

    #[test]
    fn a_recording_with_no_speech_bills_nothing() {
        // A file of pure silence must not be charged, and must not be reported
        // as an error either.
        let transcriber = Arc::new(FixedTranscriber::saying("", Some(Language::English), 0.0));
        let worker = worker_with(Some(transcriber));
        worker.submit_audio(0, encode_wav(&vec![0i16; 16_000 * 3], 16_000), "audio/wav");

        let results = collect(&worker, 1);
        assert_eq!(results[0].billable_ms(), 0);
        assert!(!results[0].is_error(), "silence is not a failure");
    }

    #[test]
    fn text_jobs_bill_nothing() {
        // A typed transcript has no audio behind it, so there is no active
        // voice to charge for.
        let worker = worker_with(None);
        worker.submit_text(0, "Push B.");
        let results = collect(&worker, 1);
        assert_eq!(results[0].billable_ms(), 0);
    }

    #[test]
    fn audio_is_transcribed_and_then_translated() {
        let transcriber = Arc::new(FixedTranscriber::saying(
            "Push B.",
            Some(Language::English),
            0.97,
        ));
        let worker = worker_with(Some(transcriber));
        assert!(worker.submit_audio(0, vec![0u8; 64], "audio/wav"));

        let results = collect(&worker, 1);
        let result = &results[0];
        assert_eq!(result.transcription.as_ref().unwrap().text, "Push B.");
        assert!(result.is_translated());
        assert_eq!(result.display_text(), Some("translated"));
    }

    #[test]
    fn text_jobs_bypass_transcription_entirely() {
        // The text path must keep working with no transcriber configured, which
        // is what lets the incoming path be developed before audio exists.
        let worker = worker_with(None);
        assert!(!worker.has_transcriber());
        assert!(worker.submit_text(0, "Push B."));

        let results = collect(&worker, 1);
        assert!(results[0].transcription.is_none());
        assert!(results[0].is_translated());
    }

    #[test]
    fn audio_without_a_transcriber_reports_a_missing_key() {
        let worker = worker_with(None);
        assert!(worker.submit_audio(0, vec![0u8; 64], "audio/wav"));

        let results = collect(&worker, 1);
        let result = &results[0];
        assert!(matches!(
            result.transcription_error,
            Some(TranscribeError::MissingKey { .. })
        ));
        assert!(result.is_error());
        assert!(result.needs_user_action());
        assert_eq!(result.outcome, None);
    }

    #[test]
    fn silence_is_skipped_rather_than_reported_as_a_failure() {
        // Game audio is mostly silence. Treating that as an error would fill the
        // UI with warnings for the normal case.
        let transcriber = Arc::new(FixedTranscriber::saying("", Some(Language::English), 0.0));
        let worker = worker_with(Some(transcriber));
        worker.submit_audio(0, vec![0u8; 64], "audio/wav");

        let results = collect(&worker, 1);
        let result = &results[0];
        assert!(result.is_skipped());
        assert!(!result.is_error());
        assert!(!result.needs_user_action());
        assert_eq!(result.outcome, None);
        assert!(result.transcription.is_some());
    }

    #[test]
    fn low_confidence_noise_is_skipped() {
        let transcriber = Arc::new(FixedTranscriber::saying("hmm", Some(Language::English), 0.05));
        let worker = worker_with(Some(transcriber));
        worker.submit_audio(0, vec![0u8; 64], "audio/wav");

        let results = collect(&worker, 1);
        assert!(results[0].is_skipped());
    }

    #[test]
    fn a_transcription_failure_is_reported_and_stops_the_chain() {
        let transcriber = Arc::new(FixedTranscriber::failing(TranscribeError::Timeout {
            after: Duration::from_secs(30),
        }));
        let worker = worker_with(Some(transcriber));
        worker.submit_audio(0, vec![0u8; 64], "audio/wav");

        let results = collect(&worker, 1);
        let result = &results[0];
        assert!(matches!(
            result.transcription_error,
            Some(TranscribeError::Timeout { .. })
        ));
        assert!(result.outcome.is_none(), "no translation should be attempted");
        assert!(!result.needs_user_action(), "a timeout is transient");
    }

    #[test]
    fn a_detected_language_matching_the_assumption_uses_one_request() {
        let transcriber = Arc::new(FixedTranscriber::saying(
            "Push B.",
            Some(Language::English),
            0.97,
        ));
        let provider = Arc::new(ScriptedTranslator::always("ok"));
        let worker = IncomingWorker::spawn(
            Some(transcriber),
            IncomingTranslator::new(provider.clone(), pair()),
        );
        worker.submit_audio(0, vec![0u8; 64], "audio/wav");
        collect(&worker, 1);

        assert_eq!(provider.call_count(), 1, "no redundant retry");
    }

    #[test]
    fn a_different_detected_language_is_translated_from_the_language_heard() {
        // The behaviour multilingual mode exists for: a teammate speaking a
        // language nobody expected must not be translated as if they had spoken
        // the assumed one.
        let transcriber = Arc::new(FixedTranscriber::saying(
            "敵が後ろにいる",
            Some(Language::Japanese),
            0.95,
        ));
        let provider = Arc::new(ScriptedTranslator::always("ok"));
        let worker = IncomingWorker::spawn(
            Some(transcriber),
            IncomingTranslator::new(provider.clone(), pair()),
        );
        worker.submit_audio(0, vec![0u8; 64], "audio/wav");
        collect(&worker, 1);

        // Two requests: the first used the assumed source, the second the one
        // that was actually heard.
        assert_eq!(provider.call_count(), 2);
        let corrected = provider.last_request().expect("a request");
        assert_eq!(corrected.source, Language::Japanese);
        assert_eq!(corrected.target, Language::Thai, "into the user's language");
    }

    #[test]
    fn an_undetected_language_leaves_the_assumption_alone() {
        let transcriber = Arc::new(FixedTranscriber::saying("Push B.", None, 0.97));
        let provider = Arc::new(ScriptedTranslator::always("ok"));
        let worker = IncomingWorker::spawn(
            Some(transcriber),
            IncomingTranslator::new(provider.clone(), pair()),
        );
        worker.submit_audio(0, vec![0u8; 64], "audio/wav");
        collect(&worker, 1);

        assert_eq!(provider.call_count(), 1, "nothing to correct");
    }

    #[test]
    fn several_jobs_are_delivered_in_order() {
        let transcriber = Arc::new(FixedTranscriber::saying(
            "Push B.",
            Some(Language::English),
            0.97,
        ));
        let worker = worker_with(Some(transcriber));
        for id in 0..5 {
            worker.submit_audio(id, vec![0u8; 32], "audio/wav");
        }

        let results = collect(&worker, 5);
        let ids: Vec<u64> = results.iter().map(|r| r.segment_id).collect();
        assert_eq!(ids, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn mixed_text_and_audio_jobs_both_work() {
        let transcriber = Arc::new(FixedTranscriber::saying(
            "Push B.",
            Some(Language::English),
            0.97,
        ));
        let worker = worker_with(Some(transcriber));
        worker.submit_text(0, "typed");
        worker.submit_audio(1, vec![0u8; 32], "audio/wav");
        worker.submit_text(2, "typed again");

        let results = collect(&worker, 3);
        assert_eq!(results.len(), 3);
        assert!(results[0].transcription.is_none());
        assert!(results[1].transcription.is_some());
        assert!(results[2].transcription.is_none());
    }

    #[test]
    fn stats_accumulate_across_the_chain() {
        let transcriber = Arc::new(FixedTranscriber::saying(
            "Push B.",
            Some(Language::English),
            0.97,
        ));
        let worker = worker_with(Some(transcriber));
        worker.submit_audio(0, vec![0u8; 32], "audio/wav");
        worker.submit_audio(1, vec![0u8; 32], "audio/wav");

        let results = collect(&worker, 2);
        assert_eq!(results[1].stats.attempts, 2);
        assert_eq!(results[1].stats.successes, 2);
    }

    #[test]
    fn the_worker_reports_itself_alive_and_stops_cleanly() {
        let worker = worker_with(None);
        assert!(worker.is_alive());
        worker.stop();
    }

    #[test]
    fn dropping_the_worker_stops_the_thread() {
        let worker = worker_with(None);
        drop(worker);
    }

    #[test]
    fn an_empty_audio_payload_still_produces_a_result() {
        // The pipeline must not silently drop a job; a zero-length payload is
        // reported as a transcription failure.
        let transcriber = Arc::new(FixedTranscriber::failing(TranscribeError::EmptyAudio));
        let worker = worker_with(Some(transcriber));
        worker.submit_audio(0, Vec::new(), "audio/wav");

        let results = collect(&worker, 1);
        assert!(matches!(
            results[0].transcription_error,
            Some(TranscribeError::EmptyAudio)
        ));
    }

    #[test]
    fn debug_output_does_not_leak_credentials() {
        let worker = worker_with(None);
        let debug = format!("{worker:?}");
        assert!(debug.contains("IncomingWorker"));
        assert!(!debug.contains("Token "));
    }

    #[test]
    fn a_result_carries_the_original_text_for_the_reader() {
        let transcriber = Arc::new(FixedTranscriber::saying(
            "Push B.",
            Some(Language::English),
            0.97,
        ));
        let worker = worker_with(Some(transcriber));
        worker.submit_audio(0, vec![0u8; 32], "audio/wav");

        let results = collect(&worker, 1);
        assert_eq!(results[0].original(), Some("Push B."));
        assert_eq!(results[0].display_text(), Some("translated"));
    }

    #[test]
    fn a_translation_failure_still_reports_the_transcript() {
        // The reader keeps the original when translation fails, and the result
        // must carry it.
        let transcriber = Arc::new(FixedTranscriber::saying(
            "Push B.",
            Some(Language::English),
            0.97,
        ));
        let provider = Arc::new(ScriptedTranslator::new());
        provider.push_err(TranslateError::Timeout {
            after: Duration::from_secs(20),
        });
        let worker = IncomingWorker::spawn(
            Some(transcriber),
            IncomingTranslator::new(provider, pair()),
        );
        worker.submit_audio(0, vec![0u8; 32], "audio/wav");

        let results = collect(&worker, 1);
        let result = &results[0];
        assert!(result.is_error());
        assert_eq!(result.original(), Some("Push B."));
        assert_eq!(result.display_text(), Some("Push B."), "kept the original");
    }

    #[test]
    fn a_transcription_language_mode_is_available_to_the_ui() {
        // Documents that the multilingual default reaches the adapter config.
        let config = crate::transcribe::deepgram::DeepgramConfig::default();
        assert_eq!(config.language, TranscriptionLanguage::Auto);
        assert!(config.listen_url().contains("language=multi"));
    }
}