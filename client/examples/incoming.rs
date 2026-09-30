//! Translate text through the real incoming pipeline and print the subtitles.
//!
//! This is the incoming path without the UI: the same
//! [`IncomingTranslator`], the same §5 prompt, the same OpenAI-compatible
//! adapter, and the same subtitle sanitizer the application uses.
//!
//! # Against a real provider
//!
//! ```bash
//! export GAME_BRIDGE_API_KEY=sk-...
//! cargo run -p game-bridge-client --example incoming -- \
//!     "Two guys pushing from B." "He's behind you!"
//! ```
//!
//! Point it at any OpenAI-compatible endpoint with:
//!
//! ```bash
//! export GAME_BRIDGE_BASE_URL=https://openrouter.ai/api/v1
//! export GAME_BRIDGE_MODEL=openai/gpt-4o-mini
//! export GAME_BRIDGE_API_KEY=sk-or-...
//! ```
//!
//! Or at a local model, which needs no key:
//!
//! ```bash
//! ollama serve
//! export GAME_BRIDGE_BASE_URL=http://127.0.0.1:11434/v1
//! export GAME_BRIDGE_MODEL=llama3.1
//! cargo run -p game-bridge-client --example incoming -- "Push B."
//! ```
//!
//! # Without a provider
//!
//! With no `GAME_BRIDGE_BASE_URL` set, it uses the built-in demo dictionary so
//! the pipeline is still exercised. That dictionary is **not translation** and
//! says so in its output.

use std::sync::Arc;
use std::time::Duration;

use game_bridge_client::translate::{
    ApiKey, DemoTranslator, IncomingTranslator, OpenAiCompatible, OpenAiConfig,
    TranslationProvider, TranslationWorker,
};
use game_bridge_client::LanguagePair;
use game_bridge_protocol::Language;

/// Default endpoint, matching the client's own default.
const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/v1";
/// Default model, matching the client's own default.
const DEFAULT_MODEL: &str = "deepseek-chat";

fn main() {
    let utterances: Vec<String> = std::env::args().skip(1).collect();
    let utterances = if utterances.is_empty() {
        // Phrases the demo dictionary actually knows, so an offline run shows
        // real lookups rather than a wall of misses.
        vec![
            "Push B.".to_string(),
            "Enemy is behind us.".to_string(),
            "Reloading.".to_string(),
            "Fall back.".to_string(),
        ]
    } else {
        utterances
    };

    // The user speaks Thai; teammates speak English. Incoming audio is
    // therefore English, translated into Thai.
    let pair = LanguagePair::new(Language::Thai, Language::English);

    // Endpoint resolution, in order of specificity:
    //   1. an explicit base URL,
    //   2. a key with no URL, which means the default provider,
    //   3. neither, which means the offline demo dictionary.
    let base_url = std::env::var("GAME_BRIDGE_BASE_URL").ok().or_else(|| {
        std::env::var("GAME_BRIDGE_API_KEY")
            .ok()
            .map(|_| DEFAULT_BASE_URL.to_string())
    });

    let (provider, description): (Arc<dyn TranslationProvider>, String) =
        match base_url {
            Some(base_url) => {
                let model = std::env::var("GAME_BRIDGE_MODEL")
                    .unwrap_or_else(|_| DEFAULT_MODEL.to_string());
                let config = OpenAiConfig {
                    base_url,
                    model,
                    timeout: Duration::from_secs(30),
                    ..Default::default()
                };
                let key = ApiKey::from_env("GAME_BRIDGE_API_KEY");

                match OpenAiCompatible::new(config, key) {
                    Ok(provider) => {
                        let description = TranslationProvider::describe(&provider);
                        (Arc::new(provider), description)
                    }
                    Err(error) => {
                        eprintln!("Cannot use that endpoint: {}", error.user_message());
                        std::process::exit(2);
                    }
                }
            }
            None => {
                eprintln!(
                    "Neither GAME_BRIDGE_BASE_URL nor GAME_BRIDGE_API_KEY is set, so this run \
                     uses the built-in demo dictionary.\nThat is a fixed phrase table, not real \
                     translation; it knows a handful of callouts and reports a miss for \
                     anything else.\n"
                );
                let provider = DemoTranslator::new(
                    // Incoming direction: teammates speak the pair's target.
                    pair.target,
                    pair.source,
                );
                let description = TranslationProvider::describe(&provider);
                (Arc::new(provider), description)
            }
        };

    println!("Provider : {description}");
    println!(
        "Direction: {} → {} (incoming)\n",
        pair.target.display_name(),
        pair.source.display_name()
    );

    let translator = IncomingTranslator::new(provider, pair)
        .with_context(Some("competitive_fps".to_string()));
    let worker = TranslationWorker::spawn(translator);

    for (index, utterance) in utterances.iter().enumerate() {
        println!(
            "  {}  [{}] {}",
            format!("{:02}", index),
            pair.target.short_tag(),
            utterance
        );
        if !worker.submit(index as u64, utterance.clone()) {
            eprintln!("Failed to queue segment {index}");
            std::process::exit(1);
        }
    }

    // Drain until every segment has produced a result.
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    let mut received = 0usize;
    let mut stats = None;
    let mut reasoning_tokens = None;

    while received < utterances.len() {
        for result in worker.drain() {
            received += 1;
            stats = Some(result.stats);
            let outcome = result.outcome;
            reasoning_tokens = outcome.reasoning_tokens;

            println!();
            if outcome.is_success() {
                println!(
                    "  {}  [{}] {}",
                    format!("{:02}", outcome.segment_id),
                    pair.source.short_tag(),
                    outcome.display_text()
                );
                println!("       {:.0} ms", outcome.latency.as_secs_f64() * 1000.0);
                if let Some(tokens) = outcome.reasoning_tokens {
                    if tokens > 0 {
                        println!("       {tokens} reasoning tokens (thinking is on)");
                    }
                }
            } else {
                // A failure is shown as the original, marked, never blanked.
                println!(
                    "  {}  [{}] {}   (untranslated)",
                    format!("{:02}", outcome.segment_id),
                    pair.target.short_tag(),
                    outcome.display_text()
                );
                if let Some(error) = &outcome.error {
                    println!("       {} — {}", error, error.user_message());
                }
            }
        }

        if received >= utterances.len() {
            break;
        }
        if std::time::Instant::now() > deadline {
            eprintln!("\nTimed out after {received} of {} segments.", utterances.len());
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    if let Some(stats) = stats {
        println!("\n{}", stats.summary());
    }
    if let Some(tokens) = reasoning_tokens {
        // Proof rather than a claim: a reasoning model left on spends hundreds
        // of tokens thinking before it answers, in front of a subtitle someone
        // is waiting to read.
        println!("reasoning tokens used: {tokens}");
    }

    worker.stop();
}