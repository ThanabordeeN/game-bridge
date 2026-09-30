# Game Bridge

**A real-time language bridge for gaming.**

Speak your language. Play with everyone.

Game Bridge captures your microphone (and optionally game audio), performs
speech-to-text, translates with game-aware context, and returns either a
subtitle overlay, a cloned-voice TTS stream into a virtual microphone, or both —
in real time, while you play.

```
Thai Mic → Game Bridge → English TTS → Game Bridge Microphone → Discord
English Voice → Game Bridge → Thai Subtitle Overlay
```

- Product specification: [`documentation/SPEC.md`](documentation/SPEC.md)
- **Incoming translation path** (what works today): [`documentation/INCOMING.md`](documentation/INCOMING.md)
- Architecture: [`documentation/ARCHITECTURE.md`](documentation/ARCHITECTURE.md)
- Wire protocol: [`documentation/PROTOCOL.md`](documentation/PROTOCOL.md)
- Design system: [`documentation/DESIGN.md`](documentation/DESIGN.md)
- MVP plan: [`documentation/MVP.md`](documentation/MVP.md)

---

## Repository layout

| Path | Contents | Language |
| --- | --- | --- |
| `client/` | Windows desktop client | Rust |
| `protocol/` | Wire protocol types + JSON schemas | Rust + JSON Schema |
| `virtual-audio-driver/` | Virtual microphone driver | C / Windows driver |
| `cloud/` | Private cloud service (not open source) | Docs only here |
| `documentation/` | Specifications and design docs | Markdown |

Open source covers the audio capture path, VAD, network protocol, virtual
microphone routing, overlay, and client UI. The cloud side — billing, wallets,
provider API keys, routing rules, fraud prevention, rate limiting — is private;
see [`cloud/README.md`](cloud/README.md).

---

## Build status — read this first

Verified on this checkout (Linux, Rust 1.98.1):

```
cargo test --workspace --features game-bridge-client/gui
  → 743 tests, 0 failures

cargo build --workspace --all-targets --features game-bridge-client/gui
  → 0 warnings, 0 errors
```

The portable Rust logic — the `protocol/` crate and the platform-independent
parts of `client/src/` (`audio/`, `routing/`, `session/`, `network/`,
`translate/`) — is real, compiles, and is covered by those tests. The egui UI
shell compiles against egui 0.29.

**The incoming path runs today, end to end.** Audio in, subtitles out, over real
HTTP, with the VAD segmenting, speech-to-text transcribing, translation, and the
active-voice meter all connected:

```bash
export DEEPGRAM_API_KEY=...              # speech-to-text
export GAME_BRIDGE_BASE_URL=https://api.inference.net/v1
export GAME_BRIDGE_MODEL=gemini-3.5-flash-lite
export GAME_BRIDGE_API_KEY=...           # translation

cargo run -p game-bridge-client -- --listen clip.wav
```

```
Game Bridge — incoming path
Direction  : English → Thai (incoming)
Speech     : nova-3 via api.deepgram.com
Translation: gemini-3.5-flash-lite via api.inference.net

--- subtitles ---
  [  6.86s]  EN  I said it before, and I'll say it again.
             TH  พูดไปแล้วและจะพูดอีกรอบ

--- session ---
  results        : 6 (4 translated, 2 skipped, 0 failed)
  active voice   : 4.810 s
  cost           : ฿0.20
```

`--listen` is the same pipeline, configuration, and session accounting as the
desktop application; only the presentation differs. It is how the product is run
on a machine with no display and no Windows.

Diagnostic harnesses, for the parts that are not a product feature:

```bash
cargo run -p game-bridge-client --example incoming      # text → translation
cargo run -p game-bridge-client --example segments -- --stt clip.wav
cargo run -p game-bridge-client --example latency -- --rounds 8 clip.wav
```

See [`documentation/INCOMING.md`](documentation/INCOMING.md),
[`documentation/LATENCY.md`](documentation/LATENCY.md), and
[`documentation/VAD.md`](documentation/VAD.md).

What is **not** verified, and is gated behind `cfg(target_os = "windows")` plus
a feature flag:

- `audio/wasapi.rs` — WASAPI capture, loopback, and playback. Written against
  the documented interfaces, **never compiled**.
- `overlay/window.rs` — the layered overlay window. Returns
  `OverlayError::Unsupported` off Windows.
- Virtual microphone driver — not written at all.
- Global hotkey registration — the state machine is tested; `RegisterHotKey` is
  not written.

> ⚠️ **This is not a working product.** There is no WebSocket transport (the
> `Transport` trait and a working in-memory implementation exist; no socket
> stack has been chosen), no gateway, no provider integration, and no driver.
> See [`documentation/MVP.md`](documentation/MVP.md) for exactly what is done
> versus planned, and why.

Host-dependent capability matrix:

| Capability | Windows | Linux (this host) |
| --- | --- | --- |
| `protocol` types, JSON codec | ✅ | ✅ tested |
| VAD, mixer, resampler, ring buffer, credits, usage | ✅ | ✅ tested |
| Routing, session, hotkey state machine, overlay model | ✅ | ✅ tested |
| **Incoming path (audio → STT → translate → subtitle)** | ✅ | ✅ **runs end to end** |
| **Active-voice meter** (billed on transcript) | ✅ | ✅ wired and tested |
| Full UI shell (compiles; never rendered here) | ✅ | ✅ compiles |
| WASAPI capture / loopback / playback | ✅ | ❌ stub, unverified |
| Virtual microphone | ✅ driver | ❌ not written |
| Overlay window | ✅ | ❌ `Unsupported` |
| Global hotkeys | ✅ `RegisterHotKey` | ❌ stub |

Non-Windows builds compile the stubs so the UI and protocol work remain
developable away from Windows. This is a development convenience, **not** a
supported product target — the product is Windows-only (§3, §44).

---

## Desktop UI stack decision

The client UI uses **egui / eframe** (immediate-mode, pure Rust, no webview).

Rationale against the §41 targets:

- **Idle RAM < 50 MB.** A WebView2-based shell (Tauri) reserves roughly
  60–150 MB before any application code loads, which makes the idle target
  unreachable. egui renders through `wgpu`/`glow` directly into a native window
  and keeps the baseline in the low tens of MB.
- **No dedicated GPU required.** egui runs on integrated graphics and software
  rasterisation fallback paths; it does not need a discrete GPU.
- **Single binary, no runtime install.** No WebView2 redistributable to ship or
  repair, which matters for a game-adjacent utility.
- **Fast startup.** Immediate-mode UI has no document model or JIT warm-up.

Tradeoff, stated plainly: egui gives less pixel-perfect control over the
card-heavy Spotify-inspired layout (§18–37) than DOM/CSS would. The design
system is therefore expressed as explicit token values in
`client/src/ui/theme.rs` (spacing, radii, type scale, surface levels) and
composed from reusable `card`, `pill_button`, and `sidebar_item` widgets so the
look stays consistent even though the layout primitives are hand-assembled.

The subtitle overlay (§32) is deliberately **not** egui: it is a separate
borderless, click-through, always-on-top layered window so it can never take
focus from the game.

---

## Getting started

### Client (Windows)

```powershell
winget install Rustlang.Rustup
rustup default stable
git clone <repo> game-bridge && cd game-bridge
cargo check --workspace
cargo run -p game-bridge-client
```

The virtual audio driver must be installed separately — the client detects its
absence and offers an install path (§40). See
[`virtual-audio-driver/README.md`](virtual-audio-driver/README.md).

### Tests

Portable logic has 743 tests that run anywhere, with no system audio libraries,
no display server, and no Windows SDK:

```bash
cargo test --workspace --features game-bridge-client/gui
```

Expected output: `743 passed; 0 failed` across the client library, two
integration suites, and the protocol crate.

The incoming path is also runnable without any of that:

```bash
cargo run -p game-bridge-client --example incoming
```

---

## Configuration

The production design is that the client never stores provider API keys (§15):
it authenticates to the Game Bridge gateway with a **short-lived, revocable,
scoped, session-specific** token, and the gateway brokers Deepgram / DeepSeek /
Cartesia / MiniMax on the user's behalf.

**The incoming translation path currently ships a bounded exception.** It speaks
directly to an OpenAI-compatible endpoint, because that is what makes the path
runnable today without building a gateway. The boundaries are enforced in code:
the key is read from an environment variable only (there is no config field that
could hold one), it is redacted from every `Debug` impl, and a remote endpoint
must be `https://`. See
[`documentation/INCOMING.md`](documentation/INCOMING.md#credentials-and-15).

Optional local config lives at
`%APPDATA%\GameBridge\config.toml` — device IDs, language pair, routing mode,
hotkeys, mixer balance, overlay styling. Provider selection is an advanced
setting, not a primary one (§46).

---

## Licensing

Client, protocol, and driver are open source (see `LICENSE`). The cloud service
is proprietary. Provider names appear in Advanced Settings only; the default UI
speaks in terms of *Subtitle*, *Voice*, and *Premium Voice* (§16, §46).
