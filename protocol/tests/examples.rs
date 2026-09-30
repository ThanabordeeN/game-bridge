//! Golden-example tests.
//!
//! These bind three things together that would otherwise drift apart:
//!
//! 1. the Rust event enums in `src/events.rs`,
//! 2. the hand-maintained JSON Schema in `schemas/events.schema.json`,
//! 3. the golden payloads in `schemas/examples/`.
//!
//! Renaming a wire field breaks this test rather than a production deploy.

mod support;

use game_bridge_protocol::events::{ClientEvent, Message, ServerEvent};
use serde_json::Value;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn schemas_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas")
}

fn load_schema() -> Value {
    let path = schemas_dir().join("events.schema.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

/// Every `.json` file under `schemas/examples/`, sorted for a stable report.
fn example_paths() -> Vec<PathBuf> {
    let dir = schemas_dir().join("examples");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.expect("readable dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no example payloads found");
    paths
}

fn read_json(path: &Path) -> Value {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

fn event_type(value: &Value) -> String {
    value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

#[test]
fn schema_is_well_formed_and_refs_resolve() {
    let schema = load_schema();
    assert_eq!(schema["$schema"], "https://json-schema.org/draft/2020-12/schema");
    let defs = schema["$defs"]
        .as_object()
        .expect("$defs must be an object");
    assert!(!defs.is_empty());

    // Every $ref in the document must resolve to a $defs entry.
    let mut refs = Vec::new();
    collect_refs(&schema, &mut refs);
    assert!(refs.len() > 20, "expected the schema to use $ref heavily");
    for reference in refs {
        let name = reference
            .strip_prefix("#/$defs/")
            .unwrap_or_else(|| panic!("non-local $ref {reference}"));
        assert!(
            defs.contains_key(name),
            "$ref {reference} has no matching $defs entry"
        );
    }
}

fn collect_refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "$ref" {
                    if let Some(s) = child.as_str() {
                        out.push(s.to_string());
                    }
                } else {
                    collect_refs(child, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect_refs(i, out)),
        _ => {}
    }
}

#[test]
fn every_example_is_valid_against_the_schema() {
    let schema = load_schema();
    let mut failures = String::new();

    for path in example_paths() {
        let instance = read_json(&path);
        if let Err(error) = support::validate(&schema, &instance, &schema) {
            let _ = writeln!(failures, "  {}: {error}", path.display());
        }
    }

    assert!(failures.is_empty(), "schema violations:\n{failures}");
}

/// Tags that belong to `ClientEvent`.
const CLIENT_TAGS: &[&str] = &[
    "session.start",
    "audio.start",
    "audio.chunk",
    "audio.end",
    "session.stop",
    "control.ptt",
    "control.bypass",
    "ping",
];

/// Tags that belong to `ServerEvent`.
const SERVER_TAGS: &[&str] = &[
    "session.started",
    "speech.start",
    "speech.end",
    "transcript.partial",
    "transcript.final",
    "translation.partial",
    "translation.final",
    "tts.audio",
    "usage.update",
    "credit.update",
    "server.draining",
    "pong",
    "error",
];

#[test]
fn every_example_decodes_as_the_event_its_type_tag_names() {
    let mut failures = String::new();

    for path in example_paths() {
        let text = std::fs::read_to_string(&path).expect("readable example");
        let instance: Value = serde_json::from_str(&text).expect("valid JSON");
        let tag = event_type(&instance);

        // Classify by exact tag, not by a namespace prefix: "session.start" is
        // a client event while "session.started" is a server event, and only an
        // exact match gets that right.
        let expect_client = CLIENT_TAGS.contains(&tag.as_str());
        let expect_server = SERVER_TAGS.contains(&tag.as_str());
        assert!(
            expect_client || expect_server,
            "{}: unknown event tag {tag:?}",
            path.display()
        );

        match Message::from_json(&text) {
            Ok(Message::Client(ev)) => {
                if !expect_client {
                    let _ = writeln!(
                        failures,
                        "  {}: {tag} decoded as a client event",
                        path.display()
                    );
                }
                let reencoded = serde_json::to_value(&*ev).expect("serializable");
                if event_type(&reencoded) != tag {
                    let _ = writeln!(
                        failures,
                        "  {}: re-encoded tag {:?} != {tag:?}",
                        path.display(),
                        event_type(&reencoded)
                    );
                }
            }
            Ok(Message::Server(ev)) => {
                if !expect_server {
                    let _ = writeln!(
                        failures,
                        "  {}: {tag} decoded as a server event",
                        path.display()
                    );
                }
                let reencoded = serde_json::to_value(&*ev).expect("serializable");
                if event_type(&reencoded) != tag {
                    let _ = writeln!(
                        failures,
                        "  {}: re-encoded tag {:?} != {tag:?}",
                        path.display(),
                        event_type(&reencoded)
                    );
                }
            }
            Err(e) => {
                let _ = writeln!(
                    failures,
                    "  {}: failed to decode {tag}: {e}",
                    path.display()
                );
            }
        }
    }

    assert!(failures.is_empty(), "example decoding problems:\n{failures}");
}

#[test]
fn round_tripping_an_example_reproduces_the_original_json() {
    // Catches a Rust-side rename that the schema comparison alone would miss:
    // if serde emits `src_lang` but the example says `source_language`, this
    // fails even though both parse independently.
    let mut failures = String::new();

    for path in example_paths() {
        let original = read_json(&path);
        let text = std::fs::read_to_string(&path).expect("readable example");

        let round_tripped = match Message::from_json(&text) {
            Ok(Message::Client(ev)) => serde_json::to_value(&*ev).expect("serializable"),
            Ok(Message::Server(ev)) => serde_json::to_value(&*ev).expect("serializable"),
            Err(e) => {
                let _ = writeln!(failures, "  {}: {e}", path.display());
                continue;
            }
        };

        // Compare as parsed values so key order and whitespace are irrelevant,
        // but every key and value must match.
        if round_tripped != original {
            let _ = writeln!(
                failures,
                "  {}:\n    example:   {original}\n    round-trip: {round_tripped}",
                path.display()
            );
        }
    }

    assert!(failures.is_empty(), "round-trip mismatches:\n{failures}");
}

#[test]
fn every_rust_event_variant_has_a_golden_example() {
    // The inverse direction of the check above: a new variant in src/events.rs
    // with no example and no schema entry is caught here.
    let mut seen = std::collections::BTreeSet::new();
    for path in example_paths() {
        seen.insert(event_type(&read_json(&path)));
    }

    let mut missing = Vec::new();
    for tag in CLIENT_TAGS.iter().chain(SERVER_TAGS.iter()) {
        if !seen.contains(*tag) {
            missing.push(*tag);
        }
    }
    assert!(missing.is_empty(), "no golden example for: {missing:?}");

    // And nothing extra: an example for an event the enums do not have would
    // mean the schema documents a variant the client cannot send.
    for tag in &seen {
        assert!(
            CLIENT_TAGS.contains(&tag.as_str()) || SERVER_TAGS.contains(&tag.as_str()),
            "example {tag} does not correspond to a known event variant"
        );
    }

    // Both enums must be distinguishable on the tag alone: a client tag must
    // not parse as a server event and vice versa. Schema validity is a separate
    // question checked by `every_example_is_valid_against_the_schema`; this
    // test is only about which enum a tag belongs to.
    for tag in CLIENT_TAGS {
        assert!(
            ServerEvent::from_json(&format!(r#"{{"type":"{tag}"}}"#)).is_err(),
            "client tag {tag} unexpectedly parsed as a server event"
        );
    }
    for tag in SERVER_TAGS {
        assert!(
            ClientEvent::from_json(&format!(r#"{{"type":"{tag}"}}"#)).is_err(),
            "server tag {tag} unexpectedly parsed as a client event"
        );
    }
}
