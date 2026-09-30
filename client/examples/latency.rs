//! Measure real provider latency through the actual adapters.
//!
//! Every other example runs once and exits, so every run pays a cold TLS
//! handshake and the numbers look far worse than what the client experiences.
//! The client keeps one pooled connection per provider for the life of a
//! session, so the number that matters is the **second and later** request.
//!
//! This probe runs each provider several times in one process and reports the
//! distribution, which is how the latency budget in `documentation/LATENCY.md`
//! was produced.
//!
//! ```bash
//! export DEEPGRAM_API_KEY=...
//! export GAME_BRIDGE_API_KEY=...
//! export GAME_BRIDGE_BASE_URL=https://api.inference.net/v1
//! export GAME_BRIDGE_MODEL=gemini-3.5-flash-lite
//!
//! cargo run -p game-bridge-client --example latency -- --rounds 5 clip.wav
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use game_bridge_client::transcribe::{
    deepgram::{Deepgram, DeepgramConfig},
    AudioInput, TranscriptionProvider,
};
use game_bridge_client::translate::{
    IncomingTranslator, OpenAiCompatible, OpenAiConfig, Secret, TranslationProvider,
};
use game_bridge_client::LanguagePair;
use game_bridge_protocol::Language;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    let mut rounds = 5usize;
    if let Some(index) = args.iter().position(|a| a == "--rounds") {
        if index + 1 < args.len() {
            rounds = args.remove(index + 1).parse().unwrap_or(5).clamp(1, 50);
            args.remove(index);
        }
    }

    let Some(path) = args.first().cloned() else {
        eprintln!("usage: latency [--rounds N] <audio-file>");
        std::process::exit(2);
    };

    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("Cannot read {path}: {error}");
            std::process::exit(2);
        }
    };

    println!("File   : {path} ({:.2} MB)", bytes.len() as f64 / 1_048_576.0);
    println!("Rounds : {rounds} (one process, so the connection is reused)\n");

    // ---- speech-to-text ----
    // Optional language pin, to compare auto-detect against a fixed language.
    let stt_config = match std::env::var("GAME_BRIDGE_STT_LANGUAGE").ok().and_then(|c| Language::from_code(&c)) {
        Some(language) => DeepgramConfig {
            language: game_bridge_client::transcribe::TranscriptionLanguage::Fixed(language),
            ..Default::default()
        },
        None => DeepgramConfig::default(),
    };
    println!("Language: {}", stt_config.language.label());

    let transcriber = match Deepgram::from_env(stt_config) {
        Ok(transcriber) => transcriber,
        Err(error) => {
            eprintln!("Speech-to-text unavailable: {}", error.user_message());
            std::process::exit(2);
        }
    };
    println!("STT    : {}", transcriber.describe());

    let input = AudioInput::Bytes {
        data: &bytes,
        content_type: "audio/wav",
    };

    let mut stt_ms = Vec::new();
    let mut transcript = String::new();
    let mut audio_seconds = 0.0f32;
    let mut detected = None;

    for round in 0..rounds {
        let started = Instant::now();
        match transcriber.transcribe(&input) {
            Ok(result) => {
                let elapsed = started.elapsed();
                stt_ms.push(elapsed.as_secs_f64() * 1000.0);
                transcript = result.text.clone();
                detected = result.detected_language;
                if let Some(duration) = result.audio_duration {
                    audio_seconds = duration.as_secs_f32();
                }
                println!(
                    "  round {}: {:>8.0} ms   (provider reported {:>6.0} ms)  conf {:.3}",
                    round + 1,
                    elapsed.as_secs_f64() * 1000.0,
                    result.latency.as_secs_f64() * 1000.0,
                    result.confidence
                );
            }
            Err(error) => {
                eprintln!("  round {}: failed — {}", round + 1, error.user_message());
                break;
            }
        }
    }

    if stt_ms.is_empty() {
        std::process::exit(1);
    }

    report("STT", &stt_ms, audio_seconds);
    if let Some(language) = detected {
        println!("  detected language: {}", language.display_name());
    }
    println!("  transcript: {}\n", truncate(&transcript, 90));

    // ---- translation ----
    let base_url = std::env::var("GAME_BRIDGE_BASE_URL").ok();
    let Some(base_url) = base_url else {
        println!("No GAME_BRIDGE_BASE_URL set; skipping the translation half.");
        return;
    };
    let model = std::env::var("GAME_BRIDGE_MODEL").unwrap_or_else(|_| "deepseek-chat".into());
    let config = OpenAiConfig {
        base_url,
        model: model.clone(),
        timeout: Duration::from_secs(60),
        ..Default::default()
    };
    let key = Secret::from_env("GAME_BRIDGE_API_KEY");

    let provider: Arc<dyn TranslationProvider> = match OpenAiCompatible::new(config, key) {
        Ok(provider) => Arc::new(provider),
        Err(error) => {
            eprintln!("Translation unavailable: {}", error.user_message());
            std::process::exit(2);
        }
    };
    println!("MT     : {}", provider.describe());

    // Translate from what was detected into the user's own language.
    let user_language = Language::Thai;
    let spoken = detected.unwrap_or(Language::English);
    let pair = LanguagePair::new(user_language, spoken);

    let mut mt_ms = Vec::new();
    let mut translator = IncomingTranslator::new(Arc::clone(&provider), pair);
    let mut reasoning_tokens = None;

    for round in 0..rounds {
        let outcome = translator.translate(round as u64, &transcript);
        mt_ms.push(outcome.latency.as_secs_f64() * 1000.0);
        reasoning_tokens = outcome.reasoning_tokens;
        println!(
            "  round {}: {:>8.0} ms   {}",
            round + 1,
            outcome.latency.as_secs_f64() * 1000.0,
            if outcome.is_success() {
                "ok".to_string()
            } else {
                format!("failed: {}", outcome.error_message().unwrap_or_default())
            }
        );
    }

    report("MT", &mt_ms, 0.0);

    // ---- summary ----
    let stt_warm = warm_median(&stt_ms);
    let mt_warm = warm_median(&mt_ms);
    println!("\n--- warm-path total ---");
    println!("  STT {stt_warm:.0} ms + translation {mt_warm:.0} ms = {:.0} ms", stt_warm + mt_warm);
    if audio_seconds > 0.0 {
        println!(
            "  for {audio_seconds:.2} s of audio, i.e. {:.2}x real time on the STT leg",
            stt_warm / 1000.0 / audio_seconds as f64
        );
    }
    if let Some(tokens) = reasoning_tokens {
        println!("  reasoning tokens: {tokens}");
    }
}

/// Median of the samples after the first, which pays connection setup.
fn warm_median(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut warm: Vec<f64> = if samples.len() > 1 {
        samples[1..].to_vec()
    } else {
        samples.to_vec()
    };
    warm.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    warm[warm.len() / 2]
}

/// Print a small distribution.
fn report(label: &str, samples: &[f64], audio_seconds: f32) {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let min = sorted.first().copied().unwrap_or(0.0);
    let max = sorted.last().copied().unwrap_or(0.0);
    let median = sorted[sorted.len() / 2];
    let cold = samples.first().copied().unwrap_or(0.0);
    let warm = warm_median(samples);

    println!("\n{label} latency over {} rounds:", samples.len());
    println!("  cold (first)  : {cold:>8.0} ms");
    println!("  warm median   : {warm:>8.0} ms");
    println!("  min / median / max: {min:.0} / {median:.0} / {max:.0} ms");
    if audio_seconds > 0.0 {
        println!("  audio length  : {audio_seconds:.2} s");
    }
    println!();
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}