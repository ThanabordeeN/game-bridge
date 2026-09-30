# Architecture

How the specification's boxes (SPEC §3, §8, §43) map onto the code in this
repository, and why the seams are where they are.

---

## Where the code lives

```
game-bridge/
├── protocol/              shared wire contract — Rust + JSON Schema
├── client/                Windows desktop client — Rust
│   └── src/
│       ├── audio/         capture, resampling, mixing, VAD, ring buffer
│       ├── network/       websocket contract, codec, reconnect, auth
│       ├── session/       lifecycle, usage history, wallet
│       ├── routing/       inbound/outbound paths, bypass
│       ├── overlay/       subtitle window and line model
│       ├── device/        endpoint discovery, virtual mic contract
│       ├── hotkeys/       push-to-translate
│       └── ui/            egui shell, theme, widgets, screens
├── virtual-audio-driver/  Windows kernel driver — C, not yet written
└── cloud/                 proprietary service — documentation only
```

`protocol/` is a separate crate rather than a module inside the client because
the *gateway* also consumes it. Two implementations of the same event format
that cannot share a definition will drift, and the drift shows up as a
production outage rather than a compile error.

---

## Threading model

The client's correctness under load depends on three kinds of thread doing
different things, and mixing them is the classic source of audio bugs.

```
┌─────────────────────┐   push, never blocks
│  WASAPI callback    │ ──────────────────────► RingBuffer (SPSC)
│  (realtime IRQL)    │
└─────────────────────┘
                                                      │ drain
                                                      ▼
┌─────────────────────┐   VAD + resample    ┌────────────────────┐
│  Audio worker       │ ◄───────────────────│  Audio worker      │
│  (normal priority)  │                     │  (normal priority) │
└─────────┬───────────┘                     └────────────────────┘
          │ speech only, framed
          ▼
┌─────────────────────┐
│  Network worker     │ ── text + binary ──► Gateway
│  (async)            │ ◄────────────────── translation, TTS
└─────────┬───────────┘
          │
          ▼
┌─────────────────────┐
│  UI thread (egui)   │  reads state, never blocks on audio or network
└─────────────────────┘
```

**The rule:** the audio callback only pushes samples into a lock-free ring. It
never allocates, never locks, and never waits. Everything expensive — VAD,
resampling, framing, sending — happens on the worker.

[`client/src/audio/ring.rs`](../client/src/audio/ring.rs) implements the SPSC
ring. On overflow it drops the **oldest** samples, so a stalled consumer loses
history rather than pushing the capture head permanently into the future.

---

## The audio path, in order

For the outbound (user's voice) direction:

| Step | Module | What happens | Why here |
| --- | --- | --- | --- |
| 1 | `audio/wasapi.rs` | WASAPI capture at the device mix format (often 48 kHz) | Device-native is cheapest |
| 2 | `audio/ring.rs` | Samples buffered lock-free | Decouples realtime callback from work |
| 3 | `audio/resampler.rs` | Resample to 16 kHz mono | The wire rate; STT providers' native input |
| 4 | `audio/vad.rs` | Detect speech, drop silence | **This is what makes "silence is not charged" true** |
| 5 | `network/websocket.rs` | Frame as `AudioFrame` and send | Binary channel, 20 ms chunks |

For the inbound (teammates' speech) direction, steps 1–4 are the same against
the application-loopback endpoint, and then:

| Step | Module | What happens |
| --- | --- | --- |
| 5 | `overlay/subtitle.rs` | Queue the transcript and translation as a timed line |
| 6 | `audio/mixer.rs` | Mix original and translated audio at the configured balance |

And for TTS returned from the gateway:

| Step | Module | What happens |
| --- | --- | --- |
| 5 | `audio/resampler.rs` | Resample from the provider's rate (22.05/24/44.1 kHz) to the endpoint's |
| 6 | `audio/mixer.rs` | Apply the bypass/mix balance |
| 7 | `device/virtual_mic.rs` | Write to the virtual microphone render endpoint |

---

## Latency budget

The end-to-end target is that a translated utterance feels conversational. The
budget below is a **design target, not a measurement** — nothing in this
repository has been profiled against real providers, and the numbers are the
allocation the design is built to rather than results.

| Stage | Target | Notes |
| --- | --- | --- |
| Capture buffering | 20 ms | `DEFAULT_CHUNK_MS` |
| VAD onset | 30–50 ms | 3 frames at 10 ms, plus pre-roll recovers the start |
| VAD hangover | 500 ms | Dominates tail latency; trades responsiveness for not splitting sentences |
| Network RTT | 20–80 ms | Region-dependent |
| STT | 150–400 ms | Streaming; partials arrive sooner |
| Translation | 150–400 ms | Depends on text length |
| TTS first chunk | 200–500 ms | Streaming; playback starts on the first chunk |
| Playback buffer | 20–60 ms | Bounded |

**The honest summary:** end-to-end is roughly **1–2 seconds** from the end of an
utterance, dominated by the hangover and the two model calls. Partials reduce
perceived latency — the subtitle can appear while the sentence is still being
spoken — but the voiced output cannot start before the sentence is understood.

The hangover is the single biggest lever and the most user-visible tradeoff.
500 ms is the default; shorter feels snappier but clips people who pause
mid-sentence.

---

## Feedback protection (§13)

The specification's most important safety property, enforced at three
independent layers:

1. **`protocol::mode::AudioSource::is_translatable_input()`** — the wire-level
   tag. Virtual-mic sources return `false`.
2. **`device::detection::DeviceInventory::for_role()`** — virtual endpoints are
   filtered out of every input list and offered only as the virtual output.
3. **`audio::assert_capturable()`** — called by both capture backends and by
   both routers before any device is opened.

Three layers rather than one because the failure mode is severe: a feedback loop
transcribes the client's own output, translates it back, and voices it louder
each round. In a headset that is a howl; in a Discord call it is a howl other
people hear.

The guarantee is also expressed structurally where possible:

- `InboundDestination` (in `routing/inbound.rs`) has **no variant** that writes
  to the virtual microphone. Re-voicing a teammate's speech into the user's mic
  is not merely discouraged; there is no type that expresses it.
- `OutboundRouter::new` refuses to construct from a virtual or render endpoint.
- `AudioDevice::is_translation_input_allowed()` is the single predicate the UI
  consults when listing inputs.

Tests assert all of this over the whole mode space, so a new routing mode cannot
quietly open a loop. See `routing/mod.rs`, `routing/inbound.rs`, and
`audio/mod.rs`.

---

## Billing correctness (§16, §29)

Two properties matter, and both are enforced in
[`protocol/src/usage.rs`](../protocol/src/usage.rs):

1. **Silence costs nothing.** `CreditRate::cost_minor_for(0) == 0` for every
   rate, asserted as a test over the whole rate table.
2. **Part minutes are pro-rata, not rounded up.** Cost is computed at
   millisecond resolution with integer arithmetic and half-up rounding. Thirty
   seconds at ฿2.50/minute is ฿1.25, not ฿2.50.

Money is **integer minor units (satang) everywhere**. Floating-point currency
accumulates drift that surfaces as a one-satang discrepancy a user screenshots.

The client's figures are a *presentation* of the gateway's numbers. `usage.update`
and `credit.update` **replace** local state rather than merging into it. A client
that could accumulate its own totals is a client that can be patched to
accumulate less; see `cloud/README.md` for the enforcement tradeoff.

---

## Device identity (§13)

Endpoints are keyed on the **stable WASAPI endpoint ID**, never on a display
name. A name is not an identity: Windows lets a user rename any endpoint, and
two identical headsets produce two entries both named "Speakers (USB Audio
Device)". A client that remembered "the one called Speakers" would, for a user
with two, silently start routing the wrong audio.

`AudioDevice` carries an `id` for identity and a `name` for display, and
`DeviceInventory::by_id` is the only lookup path. A test constructs two devices
with identical names and distinct IDs to pin this down.

---

## Provider abstraction

The provider traits in SPEC §4 and §6 (`SttProvider`, and the TTS adapter) are
**gateway-side**. The client has no provider trait and no provider code, because
it never talks to a provider. Its only outbound dependency is the WebSocket
connection to the gateway.

This is deliberate and load-bearing: the client is open source, so anything it
contains is public. A provider abstraction in the client would imply provider
credentials in the client. The seam is at the network boundary instead, and it
is why the gateway exists at all.

Swapping providers server-side changes nothing in the shipped client, which is
the §2 requirement ("the AI provider can be swapped behind the scenes")
satisfied by construction.

---

## Platform strategy

| Capability | Windows | Linux/macOS |
| --- | --- | --- |
| `protocol` crate | ✅ | ✅ |
| VAD, mixer, resampler, ring, credits, usage | ✅ | ✅ |
| Routing, session, hotkey state machine, overlay model | ✅ | ✅ |
| egui UI shell (compiles; no audio, no driver) | ✅ | ✅ |
| WASAPI capture / loopback / playback | ✅ | stub |
| Virtual microphone | ✅ driver | stub |
| Overlay window | ✅ | `Unsupported` |
| Global hotkey registration | ✅ | stub |

The portable core is not a simulation: `cargo test` exercises 421 tests with no
system audio libraries, no display server, and no Windows SDK. That is what
makes the VAD and billing logic verifiable in CI on any platform.

The Windows backends are the parts that cannot be verified here, and the README
says so plainly rather than implying the scaffold is a working product.

---

## Known limitations

Stated rather than discovered later:

1. **No WASAPI implementation.** `audio/wasapi.rs` documents the required
   interfaces and refuses to run. It has never been compiled.
2. **No WebSocket transport.** `network/websocket.rs` defines the `Transport`
   trait and ships a working in-memory implementation used by tests and the UI's
   demo mode. No socket stack has been chosen; that decision belongs with the
   first end-to-end integration.
3. **No virtual audio driver.** `virtual-audio-driver/` contains a build plan and
   an INF skeleton.
4. **Voice clone enrollment is a UI flow with no backend.**
5. **The UI has no screenshot tests.** Widget construction compiles and the
   pure helpers are tested, but nothing asserts what the window looks like.
6. **Latency figures in this document are design targets, not measurements.**
