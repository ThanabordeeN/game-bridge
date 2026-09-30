//! Text-channel codec: JSON events in, typed values out.
//!
//! Thin by design. All the shape lives in `game-bridge-protocol`, which is
//! shared with the gateway so both sides cannot drift; this module only adds
//! the client-side error mapping and the routing/session side effects that must
//! happen when particular events arrive.

use game_bridge_protocol::events::{ClientEvent, RemoteError, ServerEvent};
use game_bridge_protocol::usage::UsageUpdate;

use crate::error::NetworkError;
use crate::session::session::Session;

/// A decoded inbound message together with the effects it implies.
#[derive(Debug, Clone, PartialEq)]
pub enum Inbound {
    /// The gateway accepted the session.
    SessionStarted {
        /// Credentials to use for the rest of the session.
        credentials: game_bridge_protocol::events::SessionCredentials,
        /// Rates in force.
        rates: Vec<game_bridge_protocol::usage::CreditRate>,
    },
    /// Speech started, as detected server-side.
    SpeechStart {
        /// Segment ID.
        segment_id: u64,
    },
    /// Speech ended server-side.
    SpeechEnd {
        /// Segment ID.
        segment_id: u64,
    },
    /// Interim or final transcript.
    Transcript {
        /// Segment ID.
        segment_id: u64,
        /// Whether this is the final result for the segment.
        is_final: bool,
        /// The transcript text.
        text: String,
        /// Provider confidence.
        confidence: f32,
    },
    /// Interim or final translation.
    Translation {
        /// Segment ID.
        segment_id: u64,
        /// Whether this is the final result for the segment.
        is_final: bool,
        /// The translated text.
        text: String,
        /// Model confidence.
        confidence: f32,
    },
    /// TTS audio metadata; the audio itself follows on the binary channel.
    TtsAudio {
        /// Segment ID.
        segment_id: u64,
        /// Format of the audio that follows.
        chunk: game_bridge_protocol::events::TtsChunk,
    },
    /// Updated usage counters.
    Usage(UsageUpdate),
    /// Updated wallet balance.
    Credit {
        /// Balance in minor units.
        balance_minor: i64,
        /// Currency code.
        currency: String,
        /// Low-credit threshold, when supplied.
        threshold_minor: Option<i64>,
    },
    /// The gateway is about to close.
    Draining {
        /// Suggested delay before reconnecting.
        retry_after_ms: u32,
    },
    /// Round-trip measurement.
    Pong {
        /// Echoed client send time.
        sent_at_unix_ms: u64,
        /// Server receive time.
        received_at_unix_ms: u64,
    },
    /// A gateway-reported error.
    Error(RemoteError),
}

/// Decode a text frame from the gateway.
pub fn decode_server_event(text: &str) -> Result<Inbound, NetworkError> {
    let event = ServerEvent::from_json(text)?;
    Ok(match event {
        ServerEvent::SessionStarted {
            credentials,
            protocol_version,
            rates,
        } => {
            if protocol_version != game_bridge_protocol::PROTOCOL_VERSION {
                return Err(NetworkError::Protocol(
                    game_bridge_protocol::ProtocolError::VersionMismatch {
                        client: game_bridge_protocol::PROTOCOL_VERSION,
                        server: protocol_version,
                    },
                ));
            }
            Inbound::SessionStarted {
                credentials,
                rates,
            }
        }
        ServerEvent::SpeechStart { segment_id } => Inbound::SpeechStart { segment_id },
        ServerEvent::SpeechEnd { segment_id } => Inbound::SpeechEnd { segment_id },
        ServerEvent::TranscriptPartial {
            segment_id,
            transcript,
        } => Inbound::Transcript {
            segment_id,
            is_final: false,
            text: transcript.text,
            confidence: transcript.confidence,
        },
        ServerEvent::TranscriptFinal {
            segment_id,
            transcript,
        } => Inbound::Transcript {
            segment_id,
            is_final: true,
            text: transcript.text,
            confidence: transcript.confidence,
        },
        ServerEvent::TranslationPartial {
            segment_id,
            translation,
        } => Inbound::Translation {
            segment_id,
            is_final: false,
            text: translation.text,
            confidence: translation.confidence,
        },
        ServerEvent::TranslationFinal {
            segment_id,
            translation,
        } => Inbound::Translation {
            segment_id,
            is_final: true,
            text: translation.text,
            confidence: translation.confidence,
        },
        ServerEvent::TtsAudio { segment_id, chunk } => Inbound::TtsAudio { segment_id, chunk },
        ServerEvent::UsageUpdate { usage } => Inbound::Usage(usage),
        ServerEvent::CreditUpdate {
            balance_minor,
            currency,
            low_credit_threshold_minor,
        } => Inbound::Credit {
            balance_minor,
            currency,
            threshold_minor: low_credit_threshold_minor,
        },
        ServerEvent::ServerDraining { retry_after_ms } => Inbound::Draining { retry_after_ms },
        ServerEvent::Pong {
            sent_at_unix_ms,
            received_at_unix_ms,
        } => Inbound::Pong {
            sent_at_unix_ms,
            received_at_unix_ms,
        },
        ServerEvent::Error { error, .. } => Inbound::Error(error),
    })
}

/// Encode a client event for the text channel.
pub fn encode_client_event(event: &ClientEvent) -> Result<String, NetworkError> {
    Ok(event.to_json()?)
}

/// Apply an inbound event's effects to the session.
///
/// Returning the event as well lets the UI layer render it without duplicating
/// the mutation logic; the mutation happens here so there is exactly one place
/// where session state changes.
pub fn apply_to_session(
    session: &mut Session,
    inbound: &Inbound,
    now: std::time::Instant,
) -> Vec<SessionEffect> {
    let mut effects = Vec::new();
    match inbound {
        Inbound::SessionStarted { rates, .. } => {
            // The gateway's rate table replaces the local one.
            session.apply_rates(rates.clone());
        }
        Inbound::Usage(usage) => session.apply_usage(usage.clone()),
        Inbound::Credit {
            balance_minor,
            currency,
            threshold_minor,
        } => {
            session.apply_balance(*balance_minor, currency.clone());
            effects.push(SessionEffect::BalanceChanged {
                balance_minor: *balance_minor,
                threshold_minor: *threshold_minor,
            });
        }
        Inbound::SpeechStart { segment_id } => {
            effects.push(SessionEffect::SpeechStarted {
                segment_id: *segment_id,
            });
        }
        Inbound::SpeechEnd { segment_id } => {
            if session.close_segment(*segment_id) {
                effects.push(SessionEffect::SpeechEnded {
                    segment_id: *segment_id,
                });
            }
        }
        Inbound::Error(error) => {
            if !error.retryable {
                session.on_failed(error.message.clone(), false);
            }
            effects.push(SessionEffect::ErrorReported(error.clone()));
        }
        Inbound::Draining { retry_after_ms } => {
            session.on_disconnected(1, std::time::Duration::from_millis(u64::from(*retry_after_ms)));
            effects.push(SessionEffect::Draining {
                retry_after_ms: *retry_after_ms,
            });
        }
        Inbound::Pong { sent_at_unix_ms, .. } => {
            effects.push(SessionEffect::LatencyMeasured {
                sent_at_unix_ms: *sent_at_unix_ms,
            });
        }
        Inbound::Transcript { .. } | Inbound::Translation { .. } | Inbound::TtsAudio { .. } => {
            // Presentation-only; the UI reads these from the event itself.
        }
    }
    let _ = now;
    effects
}

/// A side effect the caller must act on, beyond session bookkeeping.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEffect {
    /// A speech segment began; the overlay should open a line.
    SpeechStarted {
        /// Segment ID.
        segment_id: u64,
    },
    /// A speech segment ended; the overlay should close the line.
    SpeechEnded {
        /// Segment ID.
        segment_id: u64,
    },
    /// A gateway error worth surfacing.
    ErrorReported(RemoteError),
    /// The wallet changed.
    BalanceChanged {
        /// New balance in minor units.
        balance_minor: i64,
        /// Threshold, when supplied.
        threshold_minor: Option<i64>,
    },
    /// The gateway is draining.
    Draining {
        /// Suggested delay before reconnecting.
        retry_after_ms: u32,
    },
    /// A pong arrived; the caller can compute round-trip time.
    LatencyMeasured {
        /// The client send time that was echoed.
        sent_at_unix_ms: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use game_bridge_protocol::language::Language;
    use game_bridge_protocol::usage::{CreditRate, ServiceTier};

    #[test]
    fn a_session_started_event_decodes_with_its_rates() {
        let event = ServerEvent::SessionStarted {
            credentials: game_bridge_protocol::events::SessionCredentials {
                session_token: "tok".into(),
                expires_at_unix: 2_000_000_000,
                session_id: "sess_1".into(),
                scopes: vec!["stt".into()],
            },
            protocol_version: game_bridge_protocol::PROTOCOL_VERSION,
            rates: CreditRate::launch_defaults(),
        };
        let inbound = decode_server_event(&event.to_json().unwrap()).unwrap();
        match inbound {
            Inbound::SessionStarted { rates, .. } => assert_eq!(rates.len(), 3),
            other => panic!("expected SessionStarted, got {other:?}"),
        }
    }

    #[test]
    fn a_version_mismatch_is_rejected_at_decode_time() {
        // Better to refuse the session than to guess at an incompatible format.
        let event = ServerEvent::SessionStarted {
            credentials: game_bridge_protocol::events::SessionCredentials {
                session_token: "tok".into(),
                expires_at_unix: 0,
                session_id: "sess_1".into(),
                scopes: vec![],
            },
            protocol_version: 99,
            rates: vec![],
        };
        let error = decode_server_event(&event.to_json().unwrap()).unwrap_err();
        assert!(matches!(
            error,
            NetworkError::Protocol(
                game_bridge_protocol::ProtocolError::VersionMismatch { server: 99, .. }
            )
        ));
    }

    #[test]
    fn partial_and_final_transcripts_are_distinguished() {
        let partial = ServerEvent::TranscriptPartial {
            segment_id: 1,
            transcript: game_bridge_protocol::events::Transcript {
                text: "ศัตรู".into(),
                language: Language::Thai,
                confidence: 0.7,
                audio_ms: 400,
                latency_ms: 100,
            },
        };
        match decode_server_event(&partial.to_json().unwrap()).unwrap() {
            Inbound::Transcript { is_final, text, .. } => {
                assert!(!is_final);
                assert_eq!(text, "ศัตรู");
            }
            other => panic!("got {other:?}"),
        }

        let final_event = ServerEvent::TranscriptFinal {
            segment_id: 1,
            transcript: game_bridge_protocol::events::Transcript {
                text: "ศัตรูอยู่ข้างหลัง".into(),
                language: Language::Thai,
                confidence: 0.97,
                audio_ms: 1200,
                latency_ms: 240,
            },
        };
        match decode_server_event(&final_event.to_json().unwrap()).unwrap() {
            Inbound::Transcript { is_final, .. } => assert!(is_final),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_produces_a_network_error_not_a_panic() {
        assert!(decode_server_event("not json").is_err());
        assert!(decode_server_event(r#"{"type":"nope"}"#).is_err());
    }

    #[test]
    fn client_events_encode_and_decode_symmetrically() {
        let event = ClientEvent::SessionStop { reason: None };
        let text = encode_client_event(&event).unwrap();
        assert_eq!(ClientEvent::from_json(&text).unwrap(), event);
    }

    #[test]
    fn usage_updates_overwrite_the_session_counters() {
        let mut session = crate::session::session::Session::new(Default::default());
        let usage = UsageUpdate {
            tier: ServiceTier::Voice,
            cost_minor: 1234,
            ..Default::default()
        };
        apply_to_session(&mut session, &Inbound::Usage(usage.clone()), std::time::Instant::now());
        assert_eq!(session.usage().cost_minor, 1234);
    }

    #[test]
    fn a_credit_update_changes_the_balance_and_reports_an_effect() {
        let mut session = crate::session::session::Session::new(Default::default());
        let effects = apply_to_session(
            &mut session,
            &Inbound::Credit {
                balance_minor: 8450,
                currency: "THB".into(),
                threshold_minor: Some(1000),
            },
            std::time::Instant::now(),
        );
        assert_eq!(session.balance_minor(), 8450);
        assert_eq!(
            effects,
            vec![SessionEffect::BalanceChanged {
                balance_minor: 8450,
                threshold_minor: Some(1000),
            }]
        );
    }

    #[test]
    fn draining_moves_the_session_to_reconnecting() {
        let mut session = crate::session::session::Session::new(Default::default());
        session.start().unwrap();
        session.on_connected(vec![]).unwrap();

        let effects = apply_to_session(
            &mut session,
            &Inbound::Draining {
                retry_after_ms: 5000,
            },
            std::time::Instant::now(),
        );
        assert!(matches!(
            session.state(),
            crate::session::session::SessionState::Reconnecting { .. }
        ));
        assert!(session.bypass().active(), "draining should force bypass");
        assert_eq!(
            effects,
            vec![SessionEffect::Draining {
                retry_after_ms: 5000
            }]
        );
    }

    #[test]
    fn a_non_retryable_error_fails_the_session() {
        let mut session = crate::session::session::Session::new(Default::default());
        session.start().unwrap();
        apply_to_session(
            &mut session,
            &Inbound::Error(RemoteError {
                code: "low_credit".into(),
                message: "Insufficient credit.".into(),
                retryable: false,
            }),
            std::time::Instant::now(),
        );
        assert!(!session.state().is_live());
        assert!(session.state().is_stopped());
    }

    #[test]
    fn a_retryable_error_does_not_fail_the_session() {
        let mut session = crate::session::session::Session::new(Default::default());
        session.start().unwrap();
        session.on_connected(vec![]).unwrap();
        apply_to_session(
            &mut session,
            &Inbound::Error(RemoteError {
                code: "provider_timeout".into(),
                message: "Translation timed out.".into(),
                retryable: true,
            }),
            std::time::Instant::now(),
        );
        assert!(session.state().is_live(), "a retryable error is recoverable");
    }

    #[test]
    fn speech_end_only_reports_an_effect_for_an_open_segment() {
        let mut session = crate::session::session::Session::new(Default::default());
        session.start().unwrap();
        let id = session.next_segment();

        let effects = apply_to_session(
            &mut session,
            &Inbound::SpeechEnd { segment_id: id },
            std::time::Instant::now(),
        );
        assert_eq!(effects, vec![SessionEffect::SpeechEnded { segment_id: id }]);

        // A duplicate end for the same segment reports nothing.
        let again = apply_to_session(
            &mut session,
            &Inbound::SpeechEnd { segment_id: id },
            std::time::Instant::now(),
        );
        assert!(again.is_empty());
    }

    #[test]
    fn a_pong_produces_a_latency_effect() {
        let mut session = crate::session::session::Session::new(Default::default());
        let effects = apply_to_session(
            &mut session,
            &Inbound::Pong {
                sent_at_unix_ms: 1_700_000_000_000,
                received_at_unix_ms: 1_700_000_000_042,
            },
            std::time::Instant::now(),
        );
        assert_eq!(
            effects,
            vec![SessionEffect::LatencyMeasured {
                sent_at_unix_ms: 1_700_000_000_000
            }]
        );
    }

    #[test]
    fn presentation_events_produce_no_session_effects() {
        let mut session = crate::session::session::Session::new(Default::default());
        let effects = apply_to_session(
            &mut session,
            &Inbound::Translation {
                segment_id: 1,
                is_final: true,
                text: "Enemy is behind us.".into(),
                confidence: 0.94,
            },
            std::time::Instant::now(),
        );
        assert!(effects.is_empty());
    }
}
