# MVP Plan

What exists, what does not, and the order the rest should be built in
(SPEC §44, §45).

---

## Honest status

The specification's MVP is 16 items. **Six are implemented and verified; three
are implemented but unverifiable on this machine; seven are not started.**

Separately, and **not** one of the 16: the incoming translation path
(text → translate → subtitle) is now built and verified end to end over real
HTTP. See [`INCOMING.md`](INCOMING.md). It was requested as a scoped phase ahead
of the MVP list, and it is the only part of the product that is runnable today.

### Implemented and verified by tests

| # | Item | Where | Evidence |
| --- | --- | --- | --- |
| — | **Incoming translation** (text → translate → subtitle) | `client/src/translate/` | 133 unit + 27 HTTP/end-to-end tests; runnable via `--example incoming` |
| — | **Speech-to-text** (audio → text, multilingual) | `client/src/transcribe/` | Verified against the real Deepgram API; `--example listen` |
| — | **Full incoming chain** (audio → STT → translate → subtitle) | `client/src/incoming.rs` | 21 tests; measured end to end in `LATENCY.md` |
| 2 | Local VAD | `client/src/audio/vad.rs` | 20 tests: onset, hangover, pre-roll, noise rejection, max-duration flush |
| 12 | Bypass mode | `client/src/routing/bypass.rs` | 13 tests incl. latch surviving a forced bypass |
| 13 | Active voice meter | `protocol/src/usage.rs` | Silence costs zero; pro-rata costing; **now wired to the pipeline and billed on the transcript** |
| 14 | Credit meter | `client/src/session/credits.rs` | 16 tests incl. the §40 low-credit threshold |
| 16 | Open-source client repository | this repository | 421 tests, zero warnings |
| — | Routing and feedback protection (§13) | `client/src/routing/` | Asserted over the whole mode space |

Components also implemented and tested, though not MVP line items: the mixer
(§12), the resampler, the SPSC ring buffer, the reconnect backoff, the overlay
line model, the hotkey state machine, the usage history, the wire protocol, and
the whole UI shell with all seven screens.

### Implemented but not verifiable here

These are real code, but this checkout is Linux with no Windows SDK, no display
server, and no audio devices. They have **never been run**.

| # | Item | Where | Status |
| --- | --- | --- | --- |
| 15 | Spotify-inspired UI | `client/src/ui/` | Compiles against egui 0.29; never rendered |
| 11 | Push-to-translate | `client/src/hotkeys/` | State machine tested; `RegisterHotKey` not written |
| 10 | Subtitle overlay | `client/src/overlay/` | Line model tested; window returns `Unsupported` off Windows |

### Not started

| # | Item | Blocker |
| --- | --- | --- |
| 1 | Capture physical microphone | Needs Windows + WASAPI |
| 3 | Stream speech to backend | Needs a transport choice and a gateway |
| 4 | Speech-to-text | **Done**: Deepgram adapter with multilingual auto-detect, verified against the real API |
| 5 | DeepSeek translation | **Done for the incoming path** via the OpenAI-compatible adapter; the gateway variant is not built |
| 6 | Cartesia TTS | Gateway-side |
| 7 | Stream translated audio back | Needs 3–6 |
| 8 | Virtual microphone | Needs the signed driver |
| 9 | Select the virtual mic in Discord | Needs 8 |

Additionally, `network/websocket.rs` defines the `Transport` trait and ships a
working in-memory implementation, but **no socket stack has been chosen**. That
decision belongs with the first end-to-end integration rather than with a
scaffold.

---

## Build order

The order below is chosen so that each step is verifiable before the next
depends on it, and so that the highest-risk items (driver signing, provider
integration) are discovered early rather than last.

### Phase 0 — Verify the scaffold on Windows

```powershell
cargo test --workspace
cargo build -p game-bridge-client --features full
```

The portable tests should pass unchanged. The `full` build is where WASAPI and
hotkey code first compiles.

**Exit criteria:** `cargo test` green on Windows; the app window opens and all
seven screens render.

### Phase 1 — Local audio in, local audio out

No cloud. Microphone → VAD → ring → playback to headphones. This proves the
realtime path and the VAD tuning on real speech, which is the part that cannot
be validated by unit tests.

- Implement `WasapiCapture` and `WasapiPlayback`.
- Wire the VAD to the activity meter so the UI shows what it is detecting.
- Measure the VAD's false-trigger rate in a real room with a mechanical keyboard.

**Exit criteria:** speaking lights the meter; typing does not; no dropouts.

### Phase 2 — The driver, in parallel from here

Start this early: signing takes days to weeks, and it is the schedule risk.

Follow `virtual-audio-driver/docs/build.md`, beginning with the unmodified
`sysvad` sample. Do not write custom driver code until the sample installs and
appears in Device Manager.

**Exit criteria:** `Game Bridge Microphone` appears, a tone written to render is
audible at capture, and the client reports the driver as present.

### Phase 3 — Transport and gateway echo

Choose a WebSocket stack, implement `Transport` for it, and stand up a gateway
that echoes. No providers yet.

- The session state machine already handles connect, drain, reconnect, and
  failure; this is what proves it against a real socket.
- Latency measurement via `ping`/`pong` becomes real here.

**Exit criteria:** a session connects, survives a gateway restart with backoff,
and reports real round-trip latency.

### Phase 4 — STT

Wire speech-to-text into the incoming path. Partials should reach the overlay
while the sentence is still being spoken.

The translation half is already done: `App::submit_transcript(segment_id, text)`
is the seam, and every test below it exercises that path.

**Exit criteria:** speech produces a correct transcript that appears as a
subtitle in the user's language.

### Phase 5 — Translation

**Done for the incoming path.** The OpenAI-compatible adapter, the §5
game-context prompt, output sanitizing, and the worker are all built and tested
over real HTTP. What remains is the gateway variant, which is a second
`TranslationProvider` implementation and changes nothing else.

### Phase 6 — TTS and the first end-to-end success

Wire Cartesia, and route the result to the virtual microphone.

**This is SPEC §45's success criterion:** a Thai speaker is heard in English by
someone else in Discord.

**Exit criteria:** met in a real Discord call, not a loopback test.

### Phase 7 — Polish to MVP

- Overlay window on Windows, with click-through and no focus theft verified
  in a real fullscreen game.
- Global hotkeys via `RegisterHotKey` and a mouse hook.
- Provider selection in Advanced Settings.
- The three-step audio test (§39).
- Benchmark against §41's targets and record the real numbers.

---

## What will change the plan

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Driver signing takes longer than expected | Item 8–9 blocked | Start Phase 2 during Phase 0; ship Subtitle mode meanwhile, since it needs no driver |
| VAD mis-tunes on real speech | False triggers bill the user for silence | Phase 1 exists to measure this before billing exists |
| Provider latency exceeds budget | Product feels unusable | Partials and streaming TTS; measure each leg from Phase 3 |
| Process loopback needs Windows 10 2004+ | Users on older Windows lose game-audio capture | Detect and fall back to whole-endpoint loopback, and say so in the UI |
| Transport choice is wrong | Rework of `network/` | The `Transport` trait is the seam; only one file changes |

---

## Deliberately out of scope for the MVP

Per SPEC §44: mobile, macOS, Linux, multiple cloned voices, team management,
professional voice cloning, self-hosted backend, local inference, plugin
marketplace.

Two additions worth stating explicitly, because they are tempting and costly:

- **A neural VAD.** The energy+ZCR detector is adequate and has no dependency or
  model. Replace it only if Phase 1's measurements show it is not.
- **A full resampling library.** The linear-interpolation resampler is adequate
  for 16 kHz speech recognition. Replace it only if recognition accuracy
  measurements show it is not.

---

## Definition of done for the MVP

A Thai speaker, in a real Discord call, speaking into their own microphone:

1. is heard by another player in English, through the virtual microphone;
2. sees the other player's English speech as Thai subtitles over the game;
3. sees a live active-voice clock, credit cost, and latency figure that match
   what they are actually being charged.

Nothing less than that is the product. Everything in this repository is either
part of that path or a test of it.
