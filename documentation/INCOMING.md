# The Incoming Path

Other players' speech → subtitles in the language the user chose.

This document covers the half of the product that is implemented: **text in,
translation out**. The outgoing half — voice-clone TTS into a virtual microphone
— is not built and is out of scope for this phase.

```
game audio ──► STT ──► transcript ──► IncomingTranslator ──► subtitle line
                        (not built)         (built)          (built)
```

Everything from the transcript onward is real, tested, and runnable today.

---

## Try it

No key, no network:

```bash
cargo run -p game-bridge-client --example incoming
```

```
Provider : demo dictionary (not real translation)
Direction: English → Thai (incoming)

  00  [EN] Push B.

  00  [TH] ดันบี
```

Against a real OpenAI-compatible endpoint:

```bash
export GAME_BRIDGE_API_KEY=sk-...
cargo run -p game-bridge-client --example incoming -- "Two guys pushing from B."
```

Against a local model, which needs no key:

```bash
ollama serve
export GAME_BRIDGE_BASE_URL=http://127.0.0.1:11434/v1
export GAME_BRIDGE_MODEL=llama3.1
cargo run -p game-bridge-client --example incoming -- "Push B."
```

---

## The direction is the reverse of the pair

This is the single most important thing to get right, and the easiest to get
silently wrong.

`LanguagePair { source, target }` is stated from the user's own point of view:
`source` is what *they* speak into the microphone, `target` is what teammates
hear. The incoming path is the opposite direction:

| | Speaks | Reads / hears |
| --- | --- | --- |
| Outgoing (not built) | user speaks `source` | teammates hear `target` |
| **Incoming (built)** | teammates speak `target` | **user reads `source`** |

Getting this backwards is invisible: the provider is asked to translate English
into English, cheerfully complies, and the user sees untranslated callouts. It
looks like a provider fault, not a wiring one.

The convention therefore lives in exactly one place —
`translate::incoming_source` and `translate::incoming_target` — and the pipeline,
the feed's language tags, and the demo provider all derive from it. Tests assert
the resulting request carries `English → Thai` for a Thai user.

---

## Module layout

| File | Responsibility |
| --- | --- |
| `translate/mod.rs` | `TranslationProvider` trait, request/response types, `TranslateError` |
| `translate/prompt.rs` | The §5 system prompt, and output sanitizing |
| `translate/openai.rs` | OpenAI-compatible `/chat/completions` adapter |
| `translate/pipeline.rs` | `IncomingTranslator`: transcript → outcome, with counters |
| `translate/worker.rs` | The background thread, so the UI never blocks |
| `translate/mock.rs` | Scripted provider for tests, demo dictionary for offline use |

---

## The §5 prompt

SPEC §5 asks for more than translation: preserve names, map locations, and
player names; understand gaming slang; shorten where appropriate; avoid
explanations. Each is a rule in the system prompt, and a test asserts every one
is present **on the bytes that leave the process** — not merely that the
function returns a string containing them.

The rules exist because a general-purpose translation prompt produces output
that is wrong *for this product* in ways that look like bugs:

- A literal rendering of a callout is too long to read mid-fight.
- A translated map name sends teammates to the wrong place.
- An explanation is useless in a one-line subtitle.

### Output is sanitized

Even told to output only the translation, models reliably add quotation marks, a
`Translation:` label, or a trailing note. `sanitize_model_output` removes the
patterns that appear in practice, including the Unicode quote forms and the
CJK/Thai label prefixes.

Two deliberate non-behaviours:

- **Trailing parentheses are preserved.** `"Push B (two of them)"` is legitimate
  callout content, and a heuristic that removed it would delete information
  mid-match.
- **Labels are cut by byte length on the original string, not the lowercased
  copy.** Lowercasing can change a character's byte length, and slicing by the
  wrong offset panics on Thai or Chinese input. There is a regression test for
  exactly that.

---

## Failure policy: the original is never discarded

When translation fails, the reader keeps the original text, marked as
untranslated. A player who reads an untranslated callout is inconvenienced; a
player who sees a blank line during a firefight has lost information they cannot
recover.

This is why `TranslationOutcome` is not a `Result`. A failure here is a
*degraded success*, and modelling it as `Result` would tempt every caller to
discard the original.

The feed distinguishes three states, and they are genuinely different:

| State | Meaning |
| --- | --- |
| `translation: Some(_)` | Translated. |
| `error: Some(_)` | Failed; the original is shown, marked `untranslated — <reason>`. |
| neither | Still in flight; shown as `translating…`. |

Conflating the last two is a real bug that shipped once during development: a
failed line rendered as `translating…` forever, and the reader could not tell it
from a slow provider.

### Only actionable failures interrupt

`TranslateError::needs_user_action` decides whether a failure raises a banner. A
missing key, a rejected key, an unusable endpoint, or an over-length segment
need the user. A timeout, a rate limit, or a connection blip do not: they are
already visible in the feed line and in the failure count, and a dialog for
every one of them would be worse than the failure.

---

## Credentials and §15

SPEC §15 says the client must never hold a provider API key, because this client
is open source and a shipped key is a leaked key. An OpenAI-compatible adapter
the client calls directly does hold one.

**This is a deliberate, bounded prototype decision.** The boundaries are
enforced in code, not left to discipline:

1. The key is read from an **environment variable only**. `ClientConfig` has no
   field capable of holding one — a test enumerates every field name and every
   string value in a serialized config and fails if either looks like a
   credential. The config stores the variable's *name*, which is not a secret.
2. `ApiKey` and `OpenAiCompatible` both have hand-written `Debug` impls that
   redact it, so a key cannot reach a log line or a crash report.
3. A non-loopback endpoint **must** be `https://`. Sending a key in clear text
   to a remote host is refused outright, and `127.0.0.1.evil.com` is correctly
   treated as remote rather than matching a naive prefix check.

The production path remains the gateway from SPEC §15, where the client holds a
short-lived session token instead. Switching to it means implementing
`TranslationProvider` once; nothing else changes.

---

## Why OpenAI-compatible

`/chat/completions` with a bearer token is not one vendor's API but a de-facto
standard. DeepSeek, OpenRouter, Groq, Together, and local runners such as Ollama
and LM Studio all implement it, so a single adapter covers cloud and offline
use — and lets the client be developed and tested with no key at all.

Endpoint URLs are normalised, because every product documents its base URL
differently and users paste what their provider told them:

| Input | Request URL |
| --- | --- |
| `https://api.deepseek.com` | `https://api.deepseek.com/v1/chat/completions` |
| `https://api.deepseek.com/v1` | `https://api.deepseek.com/v1/chat/completions` |
| `https://openrouter.ai/api/v1` | `https://openrouter.ai/api/v1/chat/completions` |
| `…/v1/chat/completions` | unchanged |

---

## Threading

Translation is a blocking network call. Running it in the UI loop would freeze
the window for as long as the provider takes, which is exactly wrong for a
client that sits over a running game.

`TranslationWorker` owns the `IncomingTranslator` outright — provider and
counters in one place, no lock — and the UI submits work and drains results once
per frame. This is also the shape the audio path needs: once STT exists, the
audio worker submits segments and nothing else changes.

```text
UI thread                     worker thread
─────────                     ─────────────
submit(id, text) ──────────►  translator.translate(id, text)
                              (blocking HTTP)
drain() ◄──────────────────   WorkResult { outcome, stats }
```

The UI repaints at 50 ms while a translation is in flight and drops back to idle
repainting when none is, so a waiting subtitle costs nothing.

---

## Configuration

Settings → Translation:

| Setting | Default |
| --- | --- |
| Provider | OpenAI-compatible |
| Base URL | `https://api.deepseek.com/v1` |
| Model | `deepseek-chat` |
| API key variable | `GAME_BRIDGE_API_KEY` |
| Timeout | 20 s |

Changing the provider restarts the worker, so the change takes effect rather
than silently doing nothing.

### Demo mode

A small built-in phrase table, for exercising the pipeline with no key and no
network. It is **not translation**: it knows a handful of callouts and reports a
miss for anything else.

It reports a miss rather than echoing the input, deliberately. Echoing with a
success status would tell the caller "translated" about text that was not
translated, and the user would see the same line twice with no indication that
nothing happened. The UI and the example both label it clearly.

---

## What is not built

| Piece | Status |
| --- | --- |
| Game audio capture (WASAPI loopback) | Not built — needs Windows |
| Speech-to-text | Not built — needs a provider |
| Feeding STT output into the pipeline | Ready: call `App::submit_transcript(segment_id, text)` |
| Subtitle overlay window | Model built and fed; the window returns `Unsupported` off Windows |
| Outgoing TTS and virtual microphone | Out of scope for this phase |

The seam for STT is a single method. `submit_transcript` is exactly what the
audio worker will call, and every test below it already exercises that path.