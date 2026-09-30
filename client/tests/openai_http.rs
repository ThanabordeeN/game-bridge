//! Integration tests for the OpenAI-compatible adapter over real HTTP.
//!
//! The unit tests in `translate::openai` cover URL normalisation, credential
//! redaction, and response *parsing* in isolation. They cannot tell whether the
//! adapter actually puts the right bytes on a socket, whether the bearer header
//! is present, or how it maps a real HTTP status.
//!
//! These tests stand up a throwaway HTTP server on loopback and drive the real
//! adapter through it. Nothing is stubbed: `ureq` opens a TCP connection, writes
//! an HTTP/1.1 request, and parses a response, exactly as it would against
//! DeepSeek. That is the difference between "the types serialise" and "the
//! adapter works".

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use game_bridge_client::translate::{
    ApiKey, OpenAiCompatible, OpenAiConfig, TranslateError, TranslationProvider,
    TranslationRequest,
};
use game_bridge_protocol::Language;

/// A canned response for the mock server to send.
struct Canned {
    status: u16,
    body: String,
    /// How long to wait before responding, for timeout tests.
    delay: Option<Duration>,
}

impl Canned {
    fn ok(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            delay: None,
        }
    }

    fn status(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            delay: None,
        }
    }
}

/// A server that handles exactly one request and reports what it received.
struct MockServer {
    base_url: String,
    received: mpsc::Receiver<String>,
}

impl MockServer {
    /// Start a server that answers one request with `canned`.
    fn start(canned: Canned) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let (mut stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(_) => return,
            };
            let request = read_http_request(&mut stream);

            if let Some(delay) = canned.delay {
                thread::sleep(delay);
            }

            let reason = match canned.status {
                200 => "OK",
                401 => "Unauthorized",
                429 => "Too Many Requests",
                500 => "Internal Server Error",
                _ => "Status",
            };
            let response = format!(
                "HTTP/1.1 {} {}\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n{}",
                canned.status,
                reason,
                canned.body.len(),
                canned.body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            let _ = tx.send(request);
        });

        Self {
            base_url: format!("http://127.0.0.1:{port}"),
            received: rx,
        }
    }

    fn config(&self, model: &str) -> OpenAiConfig {
        OpenAiConfig {
            base_url: self.base_url.clone(),
            model: model.to_string(),
            timeout: Duration::from_secs(5),
            ..Default::default()
        }
    }

    /// The raw request the server received.
    fn request(&self) -> String {
        self.received
            .recv_timeout(Duration::from_secs(5))
            .expect("the server should have received a request")
    }
}

/// Read one complete HTTP/1.1 request, honouring `Content-Length`.
fn read_http_request(stream: &mut TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];

    loop {
        let read = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buffer.extend_from_slice(&chunk[..read]);

        if let Some(header_end) = find(&buffer, b"\r\n\r\n") {
            let body_start = header_end + 4;
            let headers = String::from_utf8_lossy(&buffer[..body_start]).to_string();
            let content_length = content_length_of(&headers);
            while buffer.len() < body_start + content_length {
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

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn content_length_of(headers: &str) -> usize {
    headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
}

/// Split a raw request into its head and body.
fn split_request(raw: &str) -> (&str, &str) {
    match raw.split_once("\r\n\r\n") {
        Some((head, body)) => (head, body),
        None => (raw, ""),
    }
}

fn thai_to_english() -> TranslationRequest {
    TranslationRequest::new("ศัตรูอยู่ข้างหลัง", Language::Thai, Language::English)
}

/// A well-formed chat completion body.
fn completion(text: &str) -> String {
    format!(
        r#"{{"model":"test-model","choices":[{{"index":0,"message":{{"role":"assistant","content":{}}},"finish_reason":"stop"}}]}}"#,
        serde_json::to_string(text).expect("serialisable")
    )
}

// ---------------------------------------------------------------------------
// Happy path
// ---------------------------------------------------------------------------

#[test]
fn a_translation_is_delivered_over_real_http() {
    let server = MockServer::start(Canned::ok(completion("Enemy is behind us.")));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");

    let response = provider
        .translate(&thai_to_english())
        .expect("translation should succeed");

    assert_eq!(response.text, "Enemy is behind us.");
    assert_eq!(response.model.as_deref(), Some("test-model"));
}

#[test]
fn the_request_is_a_post_to_the_chat_completions_path() {
    let server = MockServer::start(Canned::ok(completion("ok")));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");
    provider.translate(&thai_to_english()).unwrap();

    let raw = server.request();
    let (head, _) = split_request(&raw);
    assert!(
        head.starts_with("POST /v1/chat/completions HTTP/1.1"),
        "unexpected request line: {}",
        head.lines().next().unwrap_or("")
    );
}

#[test]
fn the_request_body_carries_the_model_and_both_messages() {
    let server = MockServer::start(Canned::ok(completion("ok")));
    let provider =
        OpenAiCompatible::new(server.config("my-model"), None).expect("valid endpoint");
    provider
        .translate(&thai_to_english().with_context(Some("competitive_fps".into())))
        .unwrap();

    let raw = server.request();
    let (_, body) = split_request(&raw);
    let json: serde_json::Value = serde_json::from_str(body).expect("the body must be JSON");

    assert_eq!(json["model"], "my-model");
    assert_eq!(json["stream"], false);

    let messages = json["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 2, "a system prompt and the user's text");
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "ศัตรูอยู่ข้างหลัง");

    // The system prompt is the §5 one, not a placeholder. Compared
    // case-insensitively because the prompt capitalises the start of a rule.
    let system = messages[0]["content"].as_str().expect("system prompt");
    let lowered = system.to_lowercase();
    assert!(lowered.contains("preserve proper nouns"), "{system}");
    assert!(lowered.contains("gaming slang"), "{system}");
    assert!(lowered.contains("keep it short"), "{system}");
    assert!(lowered.contains("competitive_fps"), "{system}");
}

#[test]
fn the_system_prompt_is_sent_as_utf8_json() {
    // The prompt and the text are both non-ASCII in the general case. If the
    // body were written without an explicit charset or with escaped bytes
    // mishandled, the provider would receive mojibake.
    let server = MockServer::start(Canned::ok(completion("ok")));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");
    provider.translate(&thai_to_english()).unwrap();

    let raw = server.request();
    let (head, body) = split_request(&raw);
    assert!(
        head.to_lowercase().contains("content-type: application/json"),
        "{head}"
    );
    // The Thai text must survive as real UTF-8 bytes, not as a mangled escape.
    assert!(body.contains("ศัตรูอยู่ข้างหลัง"), "{body}");
}

// ---------------------------------------------------------------------------
// Credentials on the wire
// ---------------------------------------------------------------------------

#[test]
fn the_bearer_token_is_sent_when_a_key_is_configured() {
    let server = MockServer::start(Canned::ok(completion("ok")));
    let config = server.config("test-model");
    let provider = OpenAiCompatible::new(config, ApiKey::new("sk-test-12345"))
        .expect("valid endpoint");
    provider.translate(&thai_to_english()).unwrap();

    let raw = server.request();
    let (head, _) = split_request(&raw);
    let authorization = head
        .lines()
        .find(|line| line.to_lowercase().starts_with("authorization:"))
        .expect("an Authorization header must be present");
    assert!(
        authorization.to_lowercase().contains("bearer sk-test-12345"),
        "{authorization}"
    );
}

#[test]
fn no_authorization_header_is_sent_without_a_key() {
    // A local Ollama or LM Studio server needs no key, and sending an empty
    // bearer token can make some of them reject the request.
    let server = MockServer::start(Canned::ok(completion("ok")));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");
    provider.translate(&thai_to_english()).unwrap();

    let raw = server.request();
    let (head, _) = split_request(&raw);
    assert!(
        !head.to_lowercase().contains("authorization:"),
        "no Authorization header should be sent:\n{head}"
    );
}

// ---------------------------------------------------------------------------
// Status mapping
// ---------------------------------------------------------------------------

#[test]
fn a_401_becomes_an_unauthorized_error() {
    let server = MockServer::start(Canned::status(
        401,
        r#"{"error":{"message":"Invalid API key"}}"#,
    ));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");

    let error = provider.translate(&thai_to_english()).unwrap_err();
    assert_eq!(error, TranslateError::Unauthorized);
    assert!(!error.is_retryable(), "retrying a bad key cannot help");
    assert!(error.needs_user_action());
}

#[test]
fn a_403_becomes_an_unauthorized_error() {
    let server = MockServer::start(Canned::status(403, r#"{"error":"Forbidden"}"#));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");
    assert_eq!(
        provider.translate(&thai_to_english()).unwrap_err(),
        TranslateError::Unauthorized
    );
}

#[test]
fn a_429_becomes_a_retryable_rate_limit_error() {
    let server = MockServer::start(Canned::status(429, r#"{"error":"slow down"}"#));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");

    let error = provider.translate(&thai_to_english()).unwrap_err();
    assert!(matches!(error, TranslateError::RateLimited { .. }));
    assert!(error.is_retryable());
    assert!(
        !error.needs_user_action(),
        "rate limiting is transient and must not raise a dialog"
    );
}

#[test]
fn a_500_surfaces_the_provider_message_rather_than_a_bare_status() {
    // "HTTP 500" tells the user nothing they can act on.
    let server = MockServer::start(Canned::status(
        500,
        r#"{"error":{"message":"upstream model unavailable"}}"#,
    ));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");

    match provider.translate(&thai_to_english()).unwrap_err() {
        TranslateError::Server { status, message } => {
            assert_eq!(status, 500);
            assert!(
                message.contains("upstream model unavailable"),
                "{message}"
            );
        }
        other => panic!("expected Server, got {other:?}"),
    }
}

#[test]
fn a_400_with_a_string_error_body_is_handled() {
    let server = MockServer::start(Canned::status(400, r#"{"error":"bad request"}"#));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");
    match provider.translate(&thai_to_english()).unwrap_err() {
        TranslateError::Server { status, message } => {
            assert_eq!(status, 400);
            assert_eq!(message, "bad request");
        }
        other => panic!("expected Server, got {other:?}"),
    }
}

#[test]
fn a_non_json_error_body_does_not_panic() {
    // A provider behind a misconfigured proxy can return HTML.
    let server = MockServer::start(Canned::status(
        502,
        "<html><body>502 Bad Gateway</body></html>",
    ));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");
    assert!(matches!(
        provider.translate(&thai_to_english()).unwrap_err(),
        TranslateError::Server { status: 502, .. }
    ));
}

// ---------------------------------------------------------------------------
// Malformed success responses
// ---------------------------------------------------------------------------

#[test]
fn a_success_with_a_non_json_body_is_a_bad_response() {
    let server = MockServer::start(Canned::ok("not json at all"));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");
    assert!(matches!(
        provider.translate(&thai_to_english()).unwrap_err(),
        TranslateError::BadResponse { .. }
    ));
}

#[test]
fn a_success_with_no_choices_is_a_bad_response() {
    let server = MockServer::start(Canned::ok(r#"{"model":"x","choices":[]}"#));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");
    let error = provider.translate(&thai_to_english()).unwrap_err();
    assert!(matches!(error, TranslateError::BadResponse { .. }));
    assert!(error.to_string().contains("no message content"));
}

#[test]
fn a_null_content_is_a_bad_response_not_a_panic() {
    // A refusal or tool call returns null content.
    let server = MockServer::start(Canned::ok(
        r#"{"choices":[{"message":{"role":"assistant","content":null}}]}"#,
    ));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");
    assert!(matches!(
        provider.translate(&thai_to_english()).unwrap_err(),
        TranslateError::BadResponse { .. }
    ));
}

#[test]
fn provider_output_is_cleaned_on_the_way_back() {
    // The end-to-end version of the sanitizer test: a real HTTP response whose
    // content is labelled and quoted must reach the caller clean.
    let server = MockServer::start(Canned::ok(completion("Translation: \"Two are pushing B.\"")));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");
    let response = provider.translate(&thai_to_english()).unwrap();
    assert_eq!(response.text, "Two are pushing B.");
}

// ---------------------------------------------------------------------------
// Transport failures
// ---------------------------------------------------------------------------

#[test]
fn a_timeout_is_reported_as_a_timeout() {
    // The server accepts the connection and then stalls, which is what a
    // provider under load looks like.
    let server = MockServer::start(Canned {
        status: 200,
        body: completion("too late"),
        delay: Some(Duration::from_secs(10)),
    });
    let config = OpenAiConfig {
        timeout: Duration::from_millis(300),
        ..server.config("test-model")
    };
    let provider = OpenAiCompatible::new(config, None).expect("valid endpoint");

    let error = provider.translate(&thai_to_english()).unwrap_err();
    assert!(
        matches!(error, TranslateError::Timeout { .. }),
        "expected a timeout, got {error:?}"
    );
    assert!(error.is_retryable());
}

#[test]
fn a_refused_connection_is_reported_as_a_connect_error() {
    // Bind and immediately drop, so the port is almost certainly free.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);

    let config = OpenAiConfig {
        base_url: format!("http://127.0.0.1:{port}"),
        model: "test-model".into(),
        timeout: Duration::from_millis(500),
        ..Default::default()
    };
    let provider = OpenAiCompatible::new(config, None).expect("valid endpoint");

    let error = provider.translate(&thai_to_english()).unwrap_err();
    assert!(
        matches!(error, TranslateError::Connect { .. }),
        "expected a connect error, got {error:?}"
    );
    assert!(error.is_retryable());
}

// ---------------------------------------------------------------------------
// Provider metadata
// ---------------------------------------------------------------------------

#[test]
fn the_provider_description_identifies_the_model_and_host() {
    let server = MockServer::start(Canned::ok(completion("ok")));
    let provider =
        OpenAiCompatible::new(server.config("test-model"), None).expect("valid endpoint");
    let described = provider.describe();
    assert!(described.contains("test-model"), "{described}");
    assert!(described.contains("127.0.0.1"), "{described}");
}

#[test]
fn a_real_key_is_never_echoed_in_the_description() {
    let server = MockServer::start(Canned::ok(completion("ok")));
    let config = server.config("test-model");
    let provider =
        OpenAiCompatible::new(config, ApiKey::new("sk-must-not-appear")).expect("valid endpoint");
    assert!(!provider.describe().contains("sk-must-not-appear"));
    assert!(!format!("{provider:?}").contains("sk-must-not-appear"));
}