# Wire Protocol v1

The contract between the Game Bridge client and the gateway (SPEC §14, §15).

The authoritative definitions are:
- [`protocol/src/events.rs`](../protocol/src/events.rs) — the event enums
- [`protocol/src/frame.rs`](../protocol/src/frame.rs) — the binary audio frame
- [`protocol/schemas/events.schema.json`](../protocol/schemas/events.schema.json) — JSON Schema
- [`protocol/schemas/examples/`](../protocol/schemas/examples/) — golden payloads

Every example is verified against both the Rust types and the schema by
`cargo test -p game-bridge-protocol`. A rename that breaks the wire contract
fails a test rather than a deployment.

---

## Transport

One persistent **WebSocket over TLS** connection (`wss://`, never `ws://`).

- **Text frames** carry JSON events.
- **Binary frames** carry audio: a 16-byte header then raw PCM.

Audio is not base64-in-JSON. That would inflate the stream by ~33% and add an
encode on the client and a decode on the gateway, both on the highest-latency
path in the product.

The endpoint URL is fixed to `wss://` in `GatewayEndpoint::url()`; the scheme is
not configurable, so a build cannot be pointed at a plaintext endpoint by editing
a config value.

---

## Handshake

```
client → session.start
server → session.started      (credentials, rates)
client → audio.start          (per speech segment)
client → [binary frames]      (speech)
client → audio.end
server → speech.start / transcript.* / translation.* / tts.audio
client → session.stop
```

`session.start` must be the first message. A version mismatch is rejected at
decode time rather than guessed at:

```rust
ProtocolError::VersionMismatch { client: 1, server: 99 }
```

---

## Client events

### `session.start`

```json
{
  "type": "session.start",
  "protocol_version": 1,
  "source_language": "thai",
  "target_language": "english",
  "routing_mode": "voice_out",
  "sample_rate_hz": 16000,
  "context": "competitive_fps",
  "voice_tier": "cloned"
}
```

| Field | Required | Notes |
| --- | --- | --- |
| `protocol_version` | yes | Must equal `PROTOCOL_VERSION` |
| `source_language`, `target_language` | yes | Enum, not a free string |
| `routing_mode` | yes | `subtitle` \| `voice_out` \| `full_voice` |
| `sample_rate_hz` | yes | 16000 |
| `context` | no | Game hint for the translation prompt |
| `voice_tier` | no | `standard` \| `cloned` \| `premium`; defaults to `standard` |

### `audio.start`

Sent at VAD onset, **after** the pre-roll has been attached.

```json
{ "type": "audio.start", "source": "physical_mic", "segment_id": 0 }
```

`source` is `physical_mic` or `application_loopback`. The virtual-mic values are
never sent as inputs — `AudioSource::is_translatable_input()` returns false for
them and the routing layer cannot construct such a path.

### `audio.chunk`

Audio travels on the binary channel; this event carries timing metadata.

```json
{ "type": "audio.chunk", "segment_id": 0, "byte_len": 640, "duration_ms": 20 }
```

### `audio.end`

```json
{ "type": "audio.end", "segment_id": 0, "reason": "silence" }
```

`reason` is `silence`, `push_to_translate_released`, `user_stopped`, or
`max_duration`. The gateway treats a natural pause differently from a user stop
when finalizing the tail.

### `session.stop`

```json
{ "type": "session.stop" }
```

### `control.ptt` / `control.bypass` / `ping`

```json
{ "type": "control.ptt", "engaged": true }
{ "type": "control.bypass", "enabled": true }
{ "type": "ping", "sent_at_unix_ms": 1700000000000 }
```

---

## Server events

### `session.started`

```json
{
  "type": "session.started",
  "protocol_version": 1,
  "credentials": {
    "session_token": "gb_sess_9f2c1a4e7b3d",
    "expires_at_unix": 1700003600,
    "session_id": "sess_01HQ2X8W3Y",
    "scopes": ["stt", "translate", "tts", "usage"]
  },
  "rates": [
    { "tier": "subtitle", "minor_per_minute": 50, "currency_code": "THB" },
    { "tier": "voice", "minor_per_minute": 250, "currency_code": "THB" },
    { "tier": "premium_voice", "minor_per_minute": 400, "currency_code": "THB" }
  ]
}
```

The token is short-lived, revocable, scoped, and session-specific (SPEC §15). It
is held in memory only, is never written to disk, and its `Debug` impl redacts
it — a bearer token in a log line is a leaked credential.

**Rates in `session.started` supersede the client's local defaults for the whole
session.** Billing authority is the gateway's.

### `speech.start` / `speech.end`

```json
{ "type": "speech.start", "segment_id": 3 }
{ "type": "speech.end", "segment_id": 3 }
```

### `transcript.partial` / `transcript.final`

```json
{
  "type": "transcript.final",
  "segment_id": 3,
  "transcript": {
    "text": "ศัตรูอยู่ข้างหลัง",
    "language": "thai",
    "confidence": 0.97,
    "audio_ms": 1180,
    "latency_ms": 240
  }
}
```

`confidence` is a probability in `0.0..=1.0`. It is **rounded to three
decimals** on the wire, and out-of-range values are rejected rather than
clamped.

> Why rounding matters: `serde_json` serializes an `f32` by widening through
> `as f64`, so a raw `0.97f32` becomes `0.9700000286102295`. On a text protocol
> that is both longer than the `f64` form and unreadable to anyone
> implementing the schema from these docs. See `protocol/src/confidence.rs`.

`latency_ms` is measured from segment start, so the client can display the
honest per-leg figure rather than estimating.

### `translation.partial` / `translation.final`

```json
{
  "type": "translation.final",
  "segment_id": 3,
  "translation": {
    "text": "Enemy is behind us.",
    "confidence": 0.94,
    "language": "english",
    "latency_ms": 340
  }
}
```

### `tts.audio`

Metadata for binary frames that follow:

```json
{
  "type": "tts.audio",
  "segment_id": 3,
  "chunk": {
    "segment_id": 3,
    "sample_rate_hz": 24000,
    "channels": 1,
    "bits_per_sample": 16,
    "duration_ms": 240,
    "is_final": false
  }
}
```

The rate is declared explicitly because TTS providers do not agree on one
(22.05, 24, and 44.1 kHz are all common). The client resamples to the endpoint's
rate.

### `usage.update`

```json
{
  "type": "usage.update",
  "usage": {
    "active_voice": { "speech_ms": 744000, "session_ms": 1480000, "segments": 96 },
    "cost_minor": 620,
    "currency": "THB",
    "tier": "voice",
    "latency_ms": 340
  }
}
```

`speech_ms` is the **billed** quantity. `session_ms` is wall-clock and is never
billed. All money is integer minor units (satang).

### `credit.update`

```json
{
  "type": "credit.update",
  "balance_minor": 8450,
  "currency": "THB",
  "low_credit_threshold_minor": 1000
}
```

The threshold is optional and lets the client raise the SPEC §40 low-credit
state without polling.

### `server.draining`

```json
{ "type": "server.draining", "retry_after_ms": 5000 }
```

The client moves to `Reconnecting` and forces bypass, so the user keeps talking
while translation is unavailable.

### `pong`

```json
{
  "type": "pong",
  "sent_at_unix_ms": 1700000000000,
  "received_at_unix_ms": 1700000000042
}
```

### `error`

```json
{
  "type": "error",
  "segment_id": 12,
  "error": {
    "code": "provider_timeout",
    "message": "Translation timed out. Retrying.",
    "retryable": true
  }
}
```

`code` is stable and machine-readable; `message` is safe to show the user.
`retryable: false` fails the session; `true` does not.

Known codes: `low_credit`, `provider_timeout`, `rate_limited`,
`voice_not_enrolled`.

---

## Error semantics are split deliberately

A **session-level** error (`retryable: false`, no `segment_id`) fails the
session. A **segment-level** error with `retryable: true` lets the client keep
running and lose only that utterance. Conflating them would either drop a whole
session over one failed sentence or leave the client running against a gateway
that has already rejected it.

---

## Reconnection

Exponential backoff with jitter, capped, with a deadline
([`client/src/network/reconnect.rs`](../client/src/network/reconnect.rs)):

| Attempt | Base delay | With full jitter |
| --- | --- | --- |
| 1 | 0.5 s | 0.25–0.5 s |
| 2 | 1 s | 0.5–1 s |
| 3 | 2 s | 1–2 s |
| 4 | 4 s | 2–4 s |
| 5 | 8 s | 4–8 s |
| 6 | 16 s | 8–16 s |
| 7+ | 30 s (cap) | 15–30 s |

Jitter is not cosmetic: without it, every client that dropped when a gateway
restarted reconnects at the same instant forever. The client gives up after two
minutes and reports the failure rather than retrying silently.

While reconnecting, bypass is **forced** on, so the user's microphone keeps
reaching their friends. A latch the user set deliberately survives the outage —
see `routing/bypass.rs`, which stores the three bypass reasons as independent
flags for exactly this reason.

---

## Binary audio frame

See [`protocol/schemas/audio-frame.md`](../protocol/schemas/audio-frame.md) for
the full layout. In brief:

```
offset  size  field
     0     4  magic  "GBR1"
     4     2  version
     6     1  flags  (FINAL | TRUNCATED | TTS)
     7     1  channels
     8     4  sample_rate_hz
    12     4  payload_len
    16     N  signed 16-bit little-endian PCM
```

A 20 ms chunk at 16 kHz mono is 640 bytes of payload, 656 bytes on the wire —
about 32 kB/s while speaking, and roughly 8 kB/s for a user who talks a quarter
of the time, because VAD drops the rest.

Unknown **flag** bits are ignored, so a future version can add flags without an
older client dropping audio. Magic, version, and channel count are hard
failures.

---

## Change history

| Version | Change |
| --- | --- |
| 1 | Initial protocol. |

### Changing the protocol

1. Update `protocol/src/`.
2. Update `protocol/schemas/events.schema.json`.
3. Add or update a golden payload in `protocol/schemas/examples/`.
4. Run `cargo test -p game-bridge-protocol`.
5. Record the change here.

The schema is maintained by hand rather than generated. A generated schema would
follow any Rust change — including a breaking one — silently; a hand-maintained
one plus golden examples turns that change into a failing test a reviewer must
consciously accept.
