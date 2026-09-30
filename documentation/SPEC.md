# Game Bridge — Product Specification

**Version 0.1 · Windows · Rust client · Open-source client + managed cloud inference**

This is the specification the implementation in this repository is built
against. It is reproduced here so the repository is self-contained; where the
specification and the code disagree, the disagreement is a bug in one of them
and should be resolved explicitly rather than silently.

---

## 1. Product concept

Game Bridge is a real-time voice translation bridge for games and voice
communication.

It captures audio from the user's microphone, game/application audio, or voice
chat, then performs:

**Speech → STT → Translation → Subtitle / TTS**

and returns the result to a subtitle overlay, headphones, a virtual microphone,
or the game, Discord, or another voice application.

```
Thai mic → "ศัตรูอยู่ข้างหลัง" → Game Bridge Cloud → "Enemy is behind us"
        → Voice-clone TTS → Game Bridge Microphone → Game / Discord
```

And the other direction:

```
Player speaks English → "Push B. Two guys behind the wall."
        → "ดัน B มีสองคนอยู่หลังกำแพง" → Subtitle Overlay
```

## 2. Core product principles

- Lightweight
- Low memory usage
- Low latency
- No GPU required
- AI inference in the cloud
- Open-source client
- The user knows which audio is sent to the cloud
- Provider API keys are never in the client
- Billing counts only **active voice**
- Virtual microphone support
- Voice cloning
- The AI provider can be swapped behind the scenes

## 3. System architecture

```
┌──────────────────────────────┐
│       WINDOWS CLIENT         │
│       Rust application       │
│                              │
│  Physical mic → WASAPI capture → local VAD
│                                    │ speech only
│                              Streaming client ──► Game Bridge Cloud
│                              │                        │
│                              │                   STT → Translation
│                              │                        │
│                              │              ┌─────────┴────────┐
│                              │              ▼                  ▼
│                              │          Subtitle             TTS
│                              ◄──────────────────────────────────┘
│            ┌─────────────────┴───────────┐
│            ▼                             ▼
│      Subtitle overlay              Virtual mic
└──────────────────────────────────────────┼─────┘
                                           ▼
                                    Game / Discord
```

## 4. AI provider architecture

Providers are a modular abstraction.

**STT — default: Deepgram Nova Multilingual.** Interface:

```rust
trait SttProvider {
    async fn start_session();
    async fn send_audio();
    async fn receive_transcript();
    async fn close_session();
}
```

Also possible: Deepgram, Google, OpenAI, ElevenLabs, and a future local
provider.

> Implementation note: the trait above is specified against the *gateway*, not
> the client. The client never speaks to a provider, so it has no such trait;
> see `cloud/README.md` and `documentation/ARCHITECTURE.md`.

## 5. Translation

Default: **DeepSeek API.** The translation layer must understand game context
rather than translating literally.

Input:

```json
{
  "source_language": "th",
  "target_language": "en",
  "context": "competitive_fps",
  "text": "มันดันบีมาแล้วสอง"
}
```

Output:

```json
{ "translation": "Two are pushing B.", "confidence": 0.94 }
```

Settings: preserve names, map locations, and player names; understand gaming
slang; shorten where appropriate; avoid unnecessary explanation.

## 6. TTS

Primary: **Cartesia** — streaming TTS, voice cloning, cross-language voice
generation. Alternative: **MiniMax** for Premium Voice.

```
Translator → TTS adapter → { Cartesia, MiniMax }
```

## 7. Local VAD

VAD runs on the machine. The point:

```
Silence → DROP
Speech  → Cloud
```

Benefits: lower API cost, less bandwidth, less processing, and billing that
tracks real active voice. Requires speech start/end detection, configurable
hangover, pre-roll, and post-roll so the first and last words are not clipped.

## 8. Audio engine

WASAPI on Windows, supporting microphone capture, application audio capture via
WASAPI application loopback, playback to headphones, and the virtual microphone.

## 9. Virtual microphone

Windows must see a device named `🎙 Game Bridge Microphone`, selectable in-game
or in Discord like any other microphone.

```
Rust application → PCM → virtual render endpoint
  → virtual audio driver → virtual capture endpoint → Game Bridge Microphone
```

The driver lives in a separate repository/module. Development uses test signing
and Windows test mode; production requires production driver signing.

## 10. Voice routing modes

| Mode | Others | Me |
| --- | --- | --- |
| **A — Subtitle** | STT → translation → subtitle | untouched |
| **B — Voice Out** | subtitle | STT → translate → voice-clone TTS → virtual mic |
| **C — Full Voice** | STT → translate → TTS → headphones | STT → translate → TTS → virtual mic |

Mode B is the default. Mode C is premium.

## 11. Push-to-translate

Configurable hotkey, in either of two configurations:

```
Normal → original microphone;      Hold F8 → translate
Normal → auto translate;           Hold F8 → bypass / original voice
```

## 12. Audio mixing

Original voice 0–100%, translated voice 0–100%.

| Preset | Original | Translated |
| --- | --- | --- |
| Translation only | 0% | 100% |
| Mixed | 20% | 100% |
| Bypass | 100% | 0% |

## 13. Feedback protection

The virtual microphone must never be fed back into the translation pipeline.

```
Physical mic → translation → virtual mic
```

not

```
Virtual mic → STT → TTS → virtual mic ↺
```

Every audio endpoint must be identified by a stable device ID.

## 14. Network protocol

A persistent secure connection, recommended WebSocket + TLS.

Client events: `session.start`, `audio.start`, `audio.chunk`, `audio.end`,
`session.stop`.

Server events: `speech.start`, `speech.end`, `transcript.partial`,
`transcript.final`, `translation.partial`, `translation.final`, `tts.audio`,
`usage.update`, `credit.update`, `error`.

## 15. Authentication

The client stores no provider API key.

```
Client → (session token) → Gateway → { Deepgram, DeepSeek, Cartesia, MiniMax }
```

Tokens are short-lived, revocable, scoped, and user/session specific.

## 16. Credits

Billing is based on active voice.

- Subtitle: ฿1 = 2 active voice minutes, i.e. ฿0.50/minute
- Voice: ฿2.00–2.50 per active minute
- Premium Voice: a higher burn rate

The UI need not name the provider. The user sees Subtitle, Voice, and Premium
Voice.

## 17. Open-source strategy

Public: `client/` (Rust), `virtual-audio-driver/`, `protocol/`,
`documentation/`. Open source covers audio capture, VAD, the network protocol,
virtual microphone routing, the overlay, and the client UI.

Private cloud: billing, user wallet, API keys, provider routing rules, fraud
prevention, rate limiting.

## 18. UI direction

A **Spotify-inspired dark desktop application** — not a pixel-for-pixel copy.
Dark background, strong typography, rounded cards, minimal visual noise, large
primary actions, a persistent left sidebar, content cards, an accent colour used
for status and action, immediately readable states, and a UI built for use while
playing a game.

## 19. Main layout

```
┌─────────────────────────────────────────────────────────────┐
│ GAME BRIDGE                                      — □ ×     │
├──────────────┬──────────────────────────────────────────────┤
│ Game Bridge  │   Good evening                               │
│ ● Home       │   ┌──────────────────────────────────────┐   │
│ Translation  │   │       GAME BRIDGE                    │   │
│ Voice        │   │        ● Ready                       │   │
│ Audio        │   │       TH  →  EN                      │   │
│ ─────────    │   │       [ Start Bridge ]               │   │
│ Usage        │   └──────────────────────────────────────┘   │
│ Billing      │   Recent Sessions                            │
│ Settings     │   Valorant             42 min                │
│              │   Delta Force          63 min                │
├──────────────┴──────────────────────────────────────────────┤
│ 🎙 HyperX      TH → EN      ● Connected          ฿84.50    │
└─────────────────────────────────────────────────────────────┘
```

## 20. Sidebar

Always present on the left: GAME BRIDGE, then Home; Translation, Voice, Audio; a
divider; Usage, Billing, Settings; a divider; GitHub, About. At the bottom: user
avatar, name, and balance.

## 21. Home screen

The primary screen must allow starting within seconds.

Hero card: status (READY), the language pair with the two-letter tags, the
active mode, and a large `START GAME BRIDGE` action. Beneath it: input device,
output device, and the selected game.

## 22. Active session UI

When translation is running the hero changes state: `● LIVE`, the language pair,
a `🎙 Listening…` indicator, the active-voice clock (`12:48 active voice`), the
cost so far (`฿6.40 used`), and a `STOP BRIDGE` action.

The visualiser must be subtle — a small waveform showing that the microphone is
working, that VAD is detecting, and that the cloud is processing — not a large
spectrum analyzer.

## 23. Translation feed

A list in the style of Spotify's history:

```
LIVE TRANSLATION
19:42  EN  Two guys pushing from B.
       TH  มีสองคนกำลังดันมาจาก B
19:43  EN  He's behind you!
       TH  มันอยู่ข้างหลัง!
```

Selectable: original only, translation only, or both.

## 24. Voice screen

A card showing the current voice (avatar, name, provider tier) with a Preview
action, then a choice of Standard Voice, My Cloned Voice, or Premium Voice. The
default UI does not need to name Cartesia or MiniMax; Advanced Settings reveals
the provider.

## 25. Voice clone setup

```
Create My Voice → record sample → 10–30 seconds → upload
  → create voice → preview → save
```

## 26. Audio screen

Microphone: input and virtual output. Game audio: application. Monitoring:
headphones.

## 27. Routing UI

Spotify-style cards for Subtitle ("Text translation only"), Voice Out ("Your
voice translated into the game"), and Full Voice ("Translate everyone, both
directions"). One mode is active per session.

## 28. Usage screen

Modelled on Spotify's listening history. Each row shows session length and
active minutes as separate numbers, then the cost. A summary shows gameplay
time, active voice time, and credits used.

## 29. Wallet and billing

Balance, an add-credits action, and the current rate per tier. The screen must
state plainly: *You are charged only while speech is detected. Silence is not
charged.*

## 30. Mini player

A persistent translation control bar, shown on every screen:

```
● LIVE   Valorant     TH → EN     08:42     [■ Stop]
```

## 31. System tray

When minimised: live status, language pair, mode, active time, balance, and
Pause / Bypass Voice / Stop / Open Game Bridge actions. The client does not need
to keep its full UI open at all times.

## 32. Overlay

A minimal subtitle window showing the original and the translation. Configurable
position, opacity, font size, which text is shown, duration, and maximum lines.
It must be click-through, always-on-top, low-resource, and must never take focus
from the game.

## 33. Visual style

Layered backgrounds rather than one flat colour: base (near-black), surface,
card, hover. Spotify's exact hex values are not required.

## 34. Accent

Game Bridge has its own accent, deliberately not Spotify green, so the product is
not mistaken for a Spotify product. The accent is used only for start, active,
connected, selected, and voice-activity states. Errors are red; warnings amber.

## 35. Typography

A readable sans-serif. Hero 28–36px, section 20–24px, card title 15–18px, body
13–15px, metadata 11–13px. Bold headings with compact secondary text.

## 36. Corners

Main card 12–16px, buttons fully rounded (pill), inputs 8px, small cards
8–12px.

## 37. Interaction

Few dialogs. Settings use toggles, dropdowns, sliders, and card selection. Mode
selection looks like:

```
Translation Mode
[ Subtitle ] [ Voice Out ] [ Full Voice ]
```

## 38. First launch

Welcome → choose language → select microphone → install virtual microphone →
test audio → optional voice clone → ready.

## 39. Audio test

Three steps — microphone listening, translation, and voice playback — that
together establish that the system is ready.

## 40. Error states

Errors must name the problem and offer the fix:

- **Virtual Microphone Not Found** — "Game Bridge Audio Driver is not
  installed." with an `Install Driver` action.
- **Connection Lost** — "Translation has been paused. Reconnecting…"
- **Low Credit** — "฿4.25 remaining" with an `Add Credits` action.

## 41. Performance targets

Engineering targets, to be measured rather than assumed:

```
Idle RAM          < 50 MB
Active RAM        < 100 MB
Idle CPU          ~0%
Dedicated GPU     not required
Client startup    fast
Audio buffers     bounded
```

## 42. Rust client modules

```
client/src/
├── audio/     capture, application_loopback, microphone, playback,
│              resampler, mixer, vad
├── network/   websocket, protocol, reconnect, auth
├── session/   session, usage, credits
├── routing/   inbound, outbound, bypass
├── overlay/   window, subtitle
├── device/    detection, virtual_mic
├── hotkeys/   manager
├── ui/
└── main.rs
```

## 43. Backend services

```
Game Bridge Cloud
API Gateway
├── Authentication, Session Management, WebSocket Gateway
├── STT Service         → Deepgram
├── Translation Service → DeepSeek
├── TTS Service         → Cartesia, MiniMax
├── Credit Service, Usage Metering, Rate Limiting, Observability
```

## 44. MVP

Version 0.1 must do:

1. Capture the physical microphone
2. Local VAD
3. Stream speech to the backend
4. Deepgram STT
5. DeepSeek translation
6. Cartesia TTS
7. Stream translated audio back
8. Virtual microphone
9. Select the virtual mic in Discord or a game
10. Subtitle overlay
11. Push-to-translate
12. Bypass mode
13. Active voice meter
14. Credit meter
15. A basic Spotify-inspired UI
16. An open-source client repository

Not required for the MVP: mobile, macOS, Linux, multiple cloned voices, team
management, professional voice cloning, self-hosted backend, local inference, or
a plugin marketplace.

## 45. MVP success criteria

The prototype succeeds when:

```
Thai mic → Game Bridge → English TTS → Game Bridge Microphone → Discord
```

…and another person genuinely hears the translated voice, while:

```
English voice → Game Bridge → Thai subtitle overlay
```

…and the system shows active voice time, credit used, and current latency in
real time.

## 46. Positioning

Game Bridge is not "an AI translator application". It is:

> **A real-time language bridge for gaming.**

> **Speak your language. Play with everyone.**

The core UX is: install → select mic → select language → start bridge → play.
Provider and pipeline detail stays in Advanced Settings so the primary product
is as simple as possible.
