//! The real client binary.
//!
//! # Modes
//!
//! * **GUI** (`--features gui`, the default for a shipped build) — the desktop
//!   application. Needs a display and Windows audio.
//! * **Headless** (`--listen`) — runs the incoming pipeline over a file and
//!   prints subtitles as they are produced. No display, no audio device.
//!
//! The headless mode is not a test harness. It is the same
//! [`incoming::IncomingWorker`], the same adapters, the same configuration, and
//! the same session accounting as the GUI; only the presentation differs. That
//! makes it the one way to run the product end to end on a machine with no
//! display and no Windows — including the machine this was developed on.
//!
//! ```bash
//! export DEEPGRAM_API_KEY=...
//! export GAME_BRIDGE_BASE_URL=https://api.inference.net/v1
//! export GAME_BRIDGE_MODEL=gemini-3.5-flash-lite
//! export GAME_BRIDGE_API_KEY=...
//!
//! game-bridge --listen clip.wav
//! game-bridge --listen --config path/to/config.toml clip.wav
//! ```

use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use game_bridge_client::config::ClientConfig;
use game_bridge_client::incoming::{IncomingResult, IncomingWorker};
use game_bridge_client::secret::Secret;
use game_bridge_client::transcribe::{Deepgram, TranscriptionProvider};
use game_bridge_client::translate::{
    IncomingTranslator, OpenAiCompatible, TranslationProvider,
};
use game_bridge_client::LanguagePair;
use game_bridge_protocol::usage::CreditRate;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return ExitCode::SUCCESS;
    }

    if args.iter().any(|a| a == "--listen") {
        return match listen(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::FAILURE
            }
        };
    }

    #[cfg(feature = "gui")]
    {
        match game_bridge_client::ui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Game Bridge could not start: {error}");
                ExitCode::FAILURE
            }
        }
    }

    #[cfg(not(feature = "gui"))]
    {
        eprintln!(
            "This build has no GUI. Run it headless:\n\n    game-bridge --listen <audio file>\n\n\
             Or build the desktop application with:\n\n    cargo run -p game-bridge-client \
             --features full\n"
        );
        ExitCode::FAILURE
    }
}

fn print_usage() {
    println!(
        "Game Bridge — a real-time language bridge for gaming\n\n\
         USAGE:\n    \
         game-bridge                       start the desktop application\n    \
         game-bridge --listen <file>       transcribe and translate a recording\n\n\
         OPTIONS:\n    \
         --config <path>    read configuration from this file\n    \
         --rounds <n>       repeat the audio n times (default 1)\n    \
         -h, --help         show this message\n\n\
         ENVIRONMENT:\n    \
         DEEPGRAM_API_KEY              speech-to-text key\n    \
         GAME_BRIDGE_BASE_URL          translation endpoint\n    \
         GAME_BRIDGE_MODEL             translation model\n    \
         GAME_BRIDGE_API_KEY           translation key\n\n\
         Credentials are read from the environment only. Game Bridge never \
         writes a key to disk."
    );
}

/// Run the incoming pipeline over a file, printing subtitles as they arrive.
fn listen(args: &[String]) -> Result<(), String> {
    let path = args
        .iter()
        .skip_while(|a| *a != "--listen")
        .skip(1)
        .find(|a| !a.starts_with("--"))
        .ok_or_else(|| "usage: game-bridge --listen <audio file>".to_string())?
        .clone();

    let mut rounds = 1usize;
    if let Some(index) = args.iter().position(|a| a == "--rounds") {
        if let Some(value) = args.get(index + 1) {
            rounds = value.parse().unwrap_or(1).clamp(1, 50);
        }
    }

    // ---- configuration ----
    let config = if let Some(index) = args.iter().position(|a| a == "--config") {
        let path = args
            .get(index + 1)
            .ok_or_else(|| "--config needs a path".to_string())?;
        ClientConfig::load_or_default(std::path::Path::new(path))
            .map_err(|error| format!("Cannot read {path}: {error}"))?
    } else {
        ClientConfig::load_or_default(&ClientConfig::default_path()).unwrap_or_default()
    };

    let data = std::fs::read(&path).map_err(|error| format!("Cannot read {path}: {error}"))?;
    if data.is_empty() {
        return Err(format!("{path} is empty"));
    }

    let pair = config.session.pair;
    println!("Game Bridge — incoming path");
    println!("Source     : {path} ({} KB)", data.len() / 1024);
    println!(
        "Direction  : {} → {} (incoming)",
        pair.target.display_name(),
        pair.source.display_name()
    );
    println!("Mode       : {}", config.session.mode.label());

    // ---- speech to text ----
    let transcriber: Option<Arc<dyn TranscriptionProvider>> = {
        let key = Secret::from_env(&config.transcription.api_key_env);
        match (config.transcription.enabled, key) {
            (true, Some(key)) => {
                let stt_config = config.transcription.to_deepgram_config();
                match Deepgram::new(stt_config, Some(key)) {
                    Ok(provider) => {
                        println!("Speech     : {}", provider.describe());
                        println!("Language   : {}", config.transcription.language.label());
                        Some(Arc::new(provider))
                    }
                    Err(error) => {
                        return Err(format!(
                            "Speech-to-text is misconfigured: {}",
                            error.user_message()
                        ))
                    }
                }
            }
            _ => {
                println!(
                    "Speech     : off (set {} to enable)",
                    config.transcription.api_key_env
                );
                None
            }
        }
    };

    // ---- translation ----
    let translator_provider: Arc<dyn TranslationProvider> = {
        use game_bridge_client::config::ProviderMode;
        match config.provider.mode {
            ProviderMode::Demo => {
                let provider = game_bridge_client::translate::DemoTranslator::new(
                    pair.target,
                    pair.source,
                );
                println!(
                    "Translation: {}",
                    TranslationProvider::describe(&provider)
                );
                Arc::new(provider)
            }
            ProviderMode::OpenAi => {
                let openai = config.provider.to_openai_config();
                let key = Secret::from_env(&config.provider.api_key_env);
                let provider = OpenAiCompatible::new(openai, key)
                    .map_err(|error| format!("Translation is misconfigured: {}", error.user_message()))?;
                println!("Translation: {}", TranslationProvider::describe(&provider));
                Arc::new(provider)
            }
        }
    };

    // ---- the pipeline ----
    let translator = IncomingTranslator::new(translator_provider, pair)
        .with_context(config.session.context.clone());
    let worker = IncomingWorker::spawn(transcriber, translator);

    for round in 0..rounds {
        if !worker.submit_audio(round as u64, data.clone(), "audio/wav") {
            return Err("The pipeline stopped accepting work.".to_string());
        }
    }

    println!("\n--- subtitles ---");

    // Session accounting, exactly as the GUI does it.
    let mut speech_ms = 0u64;
    let mut received = 0usize;
    let mut translated = 0usize;
    let mut skipped = 0usize;
    let mut failures = 0usize;

    let deadline = Instant::now() + Duration::from_secs(600);
    let mut quiet_since = Instant::now();

    loop {
        let mut got_any = false;
        for result in worker.drain() {
            got_any = true;
            quiet_since = Instant::now();
            received += 1;

            // Bill what was transcribed, never what the detector guessed.
            let billable = result.billable_ms();
            speech_ms += billable;

            print_result(&result, pair, &mut translated, &mut skipped, &mut failures);
        }

        if got_any {
            continue;
        }
        if Instant::now() > deadline {
            eprintln!("Timed out after {received} results.");
            break;
        }
        // Stop once the worker has been idle for a moment and every submitted
        // round has reported at least one result.
        if received >= rounds && quiet_since.elapsed() > Duration::from_millis(750) {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    // ---- the meter, computed the same way the GUI computes it ----
    let rates = CreditRate::launch_defaults();
    let tier = game_bridge_protocol::usage::ServiceTier::for_routing_mode(config.session.mode);
    let cost_minor = CreditRate::find(&rates, tier)
        .map(|rate| rate.cost_minor_for(speech_ms))
        .unwrap_or(0);
    let currency = CreditRate::find(&rates, tier)
        .map(|rate| rate.currency_code)
        .unwrap_or_else(|| "THB".to_string());
    let symbol = game_bridge_protocol::usage::currency_symbol(&currency);

    println!("\n--- session ---");
    println!("  results        : {received} ({translated} translated, {skipped} skipped, {failures} failed)");
    println!(
        "  active voice   : {}.{:03} s",
        speech_ms / 1000,
        speech_ms % 1000
    );
    println!(
        "  billed at      : {} ({})",
        tier.label(),
        CreditRate::find(&rates, tier)
            .map(|rate| rate.display())
            .unwrap_or_else(|| "no rate".into())
    );
    println!(
        "  cost           : {symbol}{}.{:02}",
        cost_minor / 100,
        (cost_minor % 100).abs()
    );

    worker.stop();
    Ok(())
}

/// Print one result as a subtitle line.
fn print_result(
    result: &IncomingResult,
    pair: LanguagePair,
    translated: &mut usize,
    skipped: &mut usize,
    failures: &mut usize,
) {
    let at = result.start_ms as f64 / 1000.0;

    if let Some(error) = &result.transcription_error {
        *failures += 1;
        println!("  [{at:>6.2}s] ✗ {}", error.user_message());
        return;
    }

    // Nothing the provider considered speech. Normal in game audio.
    if result.is_skipped() {
        *skipped += 1;
        return;
    }

    let original = result.original().unwrap_or("");
    if original.is_empty() {
        *skipped += 1;
        return;
    }

    let detected = result
        .transcription
        .as_ref()
        .and_then(|t| t.detected_language)
        .unwrap_or(pair.target);

    println!(
        "  [{at:>6.2}s] {:>3}  {}",
        detected.short_tag(),
        original
    );

    match &result.outcome {
        Some(outcome) if outcome.is_success() => {
            *translated += 1;
            println!(
                "            {:>3}  {}",
                pair.source.short_tag(),
                outcome.display_text()
            );
            if let Some(tokens) = outcome.reasoning_tokens {
                if tokens > 0 {
                    println!("                 ({tokens} reasoning tokens)");
                }
            }
        }
        Some(outcome) => {
            *failures += 1;
            println!(
                "                 untranslated — {}",
                outcome.error_message().unwrap_or_else(|| "unknown".into())
            );
        }
        None => {
            *skipped += 1;
        }
    }
}