//! End-to-end test of the incoming path over real HTTP.
//!
//! Everything below the socket is the real thing: the real
//! [`IncomingTranslator`], the real [`TranslationWorker`] on its own thread, the
//! real [`OpenAiCompatible`] adapter writing an HTTP/1.1 request with `ureq`,
//! and the real §5 system prompt. Only the far end is a mock, and it is a mock
//! of *DeepSeek*, not of the client's own code.
//!
//! This is the test that answers "does text go in and a translation come out?"
//! without an API key.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use game_bridge_client::translate::{
    ApiKey, IncomingTranslator, OpenAiCompatible, OpenAiConfig, TranslationProvider,
    TranslationWorker,
};
use game_bridge_client::LanguagePair;
use game_bridge_protocol::Language;

/// Serve one chat completion, echoing a scripted answer per request.
///
/// Handles as many requests as `answers` provides, then stops.
fn serve(answers: Vec<String>) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        for answer in answers {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let request = read_request(&mut stream);
            let _ = tx.send(request);

            let body = format!(
                r#"{{"model":"mock","choices":[{{"message":{{"role":"assistant","content":{}}}}}]}}"#,
                serde_json::to_string(&answer).expect("serialisable")
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });

    (format!("http://127.0.0.1:{port}"), rx)
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            let body_start = position + 4;
            let headers = String::from_utf8_lossy(&buffer[..body_start]).to_string();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.eq_ignore_ascii_case("content-length") {
                        value.trim().parse().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            while buffer.len() < body_start + length {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                }
            }
            break;
        }
    }
    String::from_utf8_lossy(&buffer).to_string()
}

/// Wait for `count` results from the worker.
fn collect(worker: &TranslationWorker, count: usize) -> Vec<game_bridge_client::translate::WorkResult> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut results = Vec::new();
    while results.len() < count {
        results.extend(worker.drain());
        if results.len() >= count {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out with {} of {count} results",
            results.len()
        );
        thread::sleep(Duration::from_millis(5));
    }
    results
}

/// The user speaks Thai; teammates speak English.
fn pair() -> LanguagePair {
    LanguagePair::new(Language::Thai, Language::English)
}

#[test]
fn a_thai_user_reading_english_chat_gets_thai_subtitles() {
    // The product's incoming half, end to end: English in, Thai out.
    let (base_url, _requests) = serve(vec![
        "มีสองคนกำลังดันมาจาก B".to_string(),
        "มันอยู่ข้างหลัง!".to_string(),
    ]);

    let config = OpenAiConfig {
        base_url,
        model: "mock".into(),
        timeout: Duration::from_secs(5),
        ..Default::default()
    };
    let provider = Arc::new(OpenAiCompatible::new(config, ApiKey::new("sk-test")).unwrap());
    let translator = IncomingTranslator::new(provider, pair());
    let worker = TranslationWorker::spawn(translator);

    worker.submit(0, "Two guys pushing from B.");
    worker.submit(1, "He's behind you!");

    let results = collect(&worker, 2);
    assert_eq!(results[0].outcome.display_text(), "มีสองคนกำลังดันมาจาก B");
    assert_eq!(results[1].outcome.display_text(), "มันอยู่ข้างหลัง!");
    assert_eq!(results[1].stats.successes, 2);
}

#[test]
fn the_request_asks_the_provider_for_the_incoming_direction() {
    // The regression that would otherwise be silent: asking the provider to
    // translate English into English, which succeeds and shows untranslated
    // callouts.
    let (base_url, requests) = serve(vec!["มีสองคน".to_string()]);

    let config = OpenAiConfig {
        base_url,
        model: "mock".into(),
        timeout: Duration::from_secs(5),
        ..Default::default()
    };
    let provider = Arc::new(OpenAiCompatible::new(config, None).unwrap());
    let worker = TranslationWorker::spawn(IncomingTranslator::new(provider, pair()));
    worker.submit(0, "Two guys pushing from B.");
    collect(&worker, 1);

    let raw = requests.recv_timeout(Duration::from_secs(5)).expect("a request");
    let body = raw.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(body).expect("JSON body");
    let system = json["messages"][0]["content"].as_str().expect("system prompt");

    assert!(
        system.contains("from English into Thai"),
        "the provider was asked for the wrong direction:\n{system}"
    );
}

#[test]
fn the_section_5_prompt_reaches_the_provider_over_the_wire() {
    let (base_url, requests) = serve(vec!["ดันบี".to_string()]);

    let config = OpenAiConfig {
        base_url,
        model: "mock".into(),
        timeout: Duration::from_secs(5),
        ..Default::default()
    };
    let provider = Arc::new(OpenAiCompatible::new(config, None).unwrap());
    let translator = IncomingTranslator::new(provider, pair())
        .with_context(Some("competitive_fps".into()));
    let worker = TranslationWorker::spawn(translator);
    worker.submit(0, "Push B.");
    collect(&worker, 1);

    let raw = requests.recv_timeout(Duration::from_secs(5)).expect("a request");
    let body = raw.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    let json: serde_json::Value = serde_json::from_str(body).expect("JSON body");
    let system = json["messages"][0]["content"].as_str().expect("system prompt");
    let lowered = system.to_lowercase();

    // §5's requirements, verified on the bytes that left the process.
    assert!(lowered.contains("preserve proper nouns"), "names");
    assert!(lowered.contains("map names"), "map locations");
    assert!(lowered.contains("player names"), "player names");
    assert!(lowered.contains("gaming slang"), "slang");
    assert!(lowered.contains("keep it short"), "shortening");
    assert!(lowered.contains("no explanation"), "no explanations");
    assert!(lowered.contains("competitive_fps"), "game context");
}

#[test]
fn the_bearer_token_is_present_on_the_wire() {
    let (base_url, requests) = serve(vec!["ดันบี".to_string()]);

    let config = OpenAiConfig {
        base_url,
        model: "mock".into(),
        timeout: Duration::from_secs(5),
        ..Default::default()
    };
    let provider = Arc::new(OpenAiCompatible::new(config, ApiKey::new("sk-wire-test")).unwrap());
    let worker = TranslationWorker::spawn(IncomingTranslator::new(provider, pair()));
    worker.submit(0, "Push B.");
    collect(&worker, 1);

    let raw = requests.recv_timeout(Duration::from_secs(5)).expect("a request");
    let head = raw.split_once("\r\n\r\n").map(|(h, _)| h).unwrap_or("");
    assert!(
        head.to_lowercase().contains("authorization: bearer sk-wire-test"),
        "{head}"
    );
}

#[test]
fn a_provider_that_labels_its_answer_still_produces_a_clean_subtitle() {
    // Models add "Translation: " and quotes even when told not to. The
    // sanitizer is what keeps that out of the overlay, and this checks it
    // survives the whole round trip.
    let (base_url, _requests) = serve(vec!["Translation: \"มีสองคนดันบี\"".to_string()]);

    let config = OpenAiConfig {
        base_url,
        model: "mock".into(),
        timeout: Duration::from_secs(5),
        ..Default::default()
    };
    let provider = Arc::new(OpenAiCompatible::new(config, None).unwrap());
    let worker = TranslationWorker::spawn(IncomingTranslator::new(provider, pair()));
    worker.submit(0, "Two are pushing B.");
    let results = collect(&worker, 1);

    assert_eq!(results[0].outcome.display_text(), "มีสองคนดันบี");
}

#[test]
fn a_provider_failure_leaves_the_reader_with_the_original_callout() {
    // The degraded-success policy, over a real failed HTTP exchange.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = read_request(&mut stream);
            let body = r#"{"error":{"message":"model overloaded"}}"#;
            let response = format!(
                "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let config = OpenAiConfig {
        base_url: format!("http://127.0.0.1:{port}"),
        model: "mock".into(),
        timeout: Duration::from_secs(5),
        ..Default::default()
    };
    let provider = Arc::new(OpenAiCompatible::new(config, None).unwrap());
    let worker = TranslationWorker::spawn(IncomingTranslator::new(provider, pair()));
    worker.submit(0, "Two guys pushing from B.");
    let results = collect(&worker, 1);

    let outcome = &results[0].outcome;
    assert!(outcome.is_failure());
    assert_eq!(
        outcome.display_text(),
        "Two guys pushing from B.",
        "the original callout must survive a provider failure"
    );
    assert!(outcome.error.as_ref().unwrap().is_retryable());
    assert_eq!(results[0].stats.failures, 1);
}

#[test]
fn a_real_provider_description_identifies_the_endpoint() {
    let (base_url, _requests) = serve(vec!["x".to_string()]);
    let config = OpenAiConfig {
        base_url,
        model: "mock-model".into(),
        ..Default::default()
    };
    let provider = OpenAiCompatible::new(config, None).unwrap();
    let described = TranslationProvider::describe(&provider);
    assert!(described.contains("mock-model"), "{described}");
    assert!(described.contains("127.0.0.1"), "{described}");
    assert!(!described.contains("sk-"), "{described}");
}