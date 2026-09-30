//! Transcribe a file with the real incoming pipeline, then translate it.
//!
//! This is the full incoming chain without the UI:
//!
//! ```text
//! audio file ──► Deepgram ──► transcript ──► translation ──► subtitle
//! ```
//!
//! # Usage
//!
//! ```bash
//! export DEEPGRAM_API_KEY=...
//! export GAME_BRIDGE_API_KEY=...           # optional; without it, demo dictionary
//!
//! cargo run -p game-bridge-client --example listen -- path/to/audio.wav
//! ```
//!
//! Or point it at a hosted file Deepgram fetches itself:
//!
//! ```bash
//! cargo run -p game-bridge-client --example listen -- --url https://example.com/a.wav
//! ```
//!
//! Pin the spoken language instead of auto-detecting:
//!
//! ```bash
//! cargo run -p game-bridge-client --example listen -- --language en clip.wav
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use game_bridge_client::transcribe::{
    deepgram::{Deepgram, DeepgramConfig},
    AudioInput, TranscriptionLanguage, TranscriptionProvider,
};
use game_bridge_client::translate::{
    DemoTranslator, IncomingTranslator, OpenAiCompatible, OpenAiConfig, Secret, TranslationProvider,
};
use game_bridge_client::LanguagePair;
use game_bridge_protocol::Language;

const DEFAULT_TRANSLATE_BASE_URL: &str = "https://api.deepseek.com/v1";
const DEFAULT_TRANSLATE_MODEL: &str = "deepseek-chat";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // --language <code>
    let mut language: Option<Language> = None;
    if let Some(index) = args.iter().position(|a| a == "--language") {
        if index + 1 < args.len() {
            let code = args.remove(index + 1);
            args.remove(index);
            language = Language::from_code(&code);
            if language.is_none() {
                eprintln!("Unknown language code: {code}");
                std::process::exit(2);
            }
        }
    }

    // --url <url>
    let url_source = if let Some(index) = args.iter().position(|a| a == "--url") {
        if index + 1 >= args.len() {
            eprintln!("--url needs a value");
            std::process::exit(2);
        }
        Some(args.remove(index + 1))
    } else {
        None
    };

    // `is_url` is captured before the Option is consumed, because whether the
    // audio came from a URL decides how it is sent.
    let is_url = url_source.is_some();
    let source = url_source.or_else(|| args.first().cloned());
    let Some(source) = source else {
        eprintln!(
            "usage: listen [--language <code>] <audio-file>\n       listen [--language <code>] \
             --url <https://...>"
        );
        std::process::exit(2);
    };

    // ---- transcription ----
    let stt_config = DeepgramConfig {
        language: match language {
            Some(language) => TranscriptionLanguage::Fixed(language),
            None => TranscriptionLanguage::Auto,
        },
        ..Default::default()
    };
    let transcriber = match Deepgram::from_env(stt_config) {
        Ok(transcriber) => transcriber,
        Err(error) => {
            eprintln!("Cannot configure speech-to-text: {}", error.user_message());
            std::process::exit(2);
        }
    };

    println!("Speech-to-text : {}", transcriber.describe());
    println!("Language mode  : {}", transcriber.config().language.label());

    let audio_bytes = if is_url {
        Vec::new()
    } else {
        match std::fs::read(&source) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("Cannot read {source}: {error}");
                std::process::exit(2);
            }
        }
    };

    let input = if is_url {
        AudioInput::Url(&source)
    } else {
        AudioInput::Bytes {
            data: &audio_bytes,
            content_type: mime_for(&source),
        }
    };

    println!(
        "Audio          : {} ({:.2} MB)\n",
        source,
        input.len() as f64 / 1_048_576.0
    );

    let started = Instant::now();
    let transcription = match transcriber.transcribe(&input) {
        Ok(transcription) => transcription,
        Err(error) => {
            eprintln!("Transcription failed: {}", error.user_message());
            eprintln!("  ({error})");
            std::process::exit(1);
        }
    };
    let wall = started.elapsed();

    println!("--- transcript ---");
    println!("{}", transcription.text);
    println!();
    println!("confidence        : {:.3}", transcription.confidence);
    match transcription.detected_language {
        Some(language) => println!(
            "detected language : {} ({})",
            language.display_name(),
            transcription
                .language_confidence
                .map(|c| format!("{c:.3}"))
                .unwrap_or_else(|| "n/a".into())
        ),
        None => println!("detected language : not reported"),
    }
    if let Some(duration) = transcription.audio_duration {
        println!("audio duration    : {:.2} s", duration.as_secs_f32());
    }
    println!(
        "stt latency       : {} ms (provider reported {} ms)",
        wall.as_millis(),
        transcription.latency.as_millis()
    );

    if !game_bridge_client::transcribe::is_translatable(&transcription) {
        println!("\nNothing worth translating (empty or low-confidence).");
        return;
    }

    // ---- translation ----
    // The source language is what was *detected*, not what was assumed. That is
    // the point of multilingual mode: a teammate who speaks a language nobody
    // expected is still translated from the right one.
    let user_language = Language::Thai;
    let spoken = transcription.detected_language.unwrap_or(user_language);

    if spoken == user_language {
        println!(
            "\nDetected language is already the user's own ({}); nothing to translate.",
            user_language.display_name()
        );
        return;
    }

    let pair = LanguagePair::new(user_language, spoken);

    let (provider, description): (Arc<dyn TranslationProvider>, String) =
        match resolve_translation_endpoint() {
            Some((base_url, model)) => {
                let config = OpenAiConfig {
                    base_url,
                    model,
                    timeout: Duration::from_secs(30),
                    ..Default::default()
                };
                let key = Secret::from_env("GAME_BRIDGE_API_KEY");
                match OpenAiCompatible::new(config, key) {
                    Ok(provider) => {
                        let described = TranslationProvider::describe(&provider);
                        (Arc::new(provider), described)
                    }
                    Err(error) => {
                        eprintln!("Cannot use that translation endpoint: {}", error.user_message());
                        std::process::exit(2);
                    }
                }
            }
            None => {
                eprintln!(
                    "(no translation endpoint configured; using the demo dictionary, which is \
                     not real translation)"
                );
                let provider = DemoTranslator::new(spoken, user_language);
                let described = TranslationProvider::describe(&provider);
                (Arc::new(provider), described)
            }
        };

    println!("\nTranslation    : {description}");
    println!(
        "Direction      : {} → {} (incoming)\n",
        spoken.display_name(),
        user_language.display_name()
    );

    // `IncomingTranslator` derives the direction from the pair, so the pair is
    // stated from the user's point of view: they speak `user_language`.
    let translator = IncomingTranslator::new(provider, pair)
        .with_context(Some("competitive_fps".to_string()));

    let outcome = {
        let mut translator = translator;
        translator.translate(0, &transcription.text)
    };

    println!("--- subtitle ---");
    println!("  [{}] {}", spoken.short_tag(), outcome.original);
    match &outcome.translation {
        Some(translated) => println!("  [{}] {}", user_language.short_tag(), translated),
        None => println!("  (untranslated — kept the original)"),
    }
    println!("\ntranslation latency: {} ms", outcome.latency.as_millis());
    if let Some(tokens) = outcome.reasoning_tokens {
        println!("reasoning tokens   : {tokens}");
    }
    println!(
        "\ntotal (stt + translate): {} ms",
        wall.as_millis() + outcome.latency.as_millis()
    );
}

/// Pick a translation endpoint from the environment.
fn resolve_translation_endpoint() -> Option<(String, String)> {
    let model = std::env::var("GAME_BRIDGE_MODEL").unwrap_or_else(|_| DEFAULT_TRANSLATE_MODEL.into());
    std::env::var("GAME_BRIDGE_BASE_URL")
        .ok()
        .or_else(|| {
            std::env::var("GAME_BRIDGE_API_KEY")
                .ok()
                .map(|_| DEFAULT_TRANSLATE_BASE_URL.to_string())
        })
        .map(|base_url| (base_url, model))
}

/// Guess a MIME type from the file extension.
///
/// Deepgram needs the container format, not just the bytes.
fn mime_for(path: &str) -> &'static str {
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