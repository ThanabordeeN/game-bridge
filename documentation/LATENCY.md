# Latency

Measured numbers, not targets. Everything here was produced by
`cargo run -p game-bridge-client --example latency` against the real providers
from this checkout, on a normal consumer connection.

The measurement harness is in the repository so these numbers can be re-derived
rather than taken on trust.

---

## The headline

| Stage | Cold (first request) | Warm (steady state) |
| --- | --- | --- |
| Speech-to-text | **2.1–3.0 s** | **0.32–0.58 s** |
| Translation | 2.4–7.8 s | **1.0–1.8 s** |
| **Total per utterance** | ~5–10 s | **~1.4–2.4 s** |

The single most important number here is the gap between cold and warm. **A
fresh TLS handshake costs more than the transcription itself.** Keeping the
connection alive is worth roughly 1.8 seconds per callout, which is the
difference between a product that feels broken and one that feels usable.

That is what SPEC §14's "persistent secure connection" buys, and it is why both
adapters hold a pooled `ureq::Agent` rather than opening a connection per
request.

---

## Speech-to-text

Deepgram `nova-3`, multilingual (`language=multi&detect_language=true`), eight
rounds against a five-second clip of real speech:

```
round 1:  2193 ms   <- cold: TLS handshake
round 2:   583 ms
round 3:   357 ms
round 4:   321 ms
round 5:   339 ms
round 6:   321 ms
round 7:   350 ms
round 8:   330 ms

cold: 2193 ms   warm median: 339 ms
```

### Multilingual auto-detect costs nothing

The obvious worry with `language=multi` plus `detect_language=true` is that
detection costs latency. Measured, it does not:

| Language mode | Warm median |
| --- | --- |
| Auto (multilingual + detect) | **339 ms** |
| Pinned `en` | **364 ms** |

The difference is within noise. Auto-detection is therefore the default: a
gaming lobby is not one language, and the feature is free.

### Duration matters less than variance

| Audio | Warm samples | Median |
| --- | --- | --- |
| 2 s | 899, 965, 686 | 899 ms |
| 3 s | 872, 411, 426 | 426 ms |
| 5 s | 719, 389, 1019 | 719 ms |
| 8.6 s | 669, 2124, 291 | 669 ms |

In the 2–10 s range, **duration is not the dominant term** — provider queueing
variance is. The same 8.6-second clip took 291 ms and 2124 ms in consecutive
requests.

For a real-time product this matters more than the median: a subtitle that
*sometimes* takes 2.1 s is worse than one that always takes 0.7 s, because the
user cannot tell a slow response from a broken one.

### A measurement that was wrong, and why

An earlier pass measured "latency against audio duration" by trimming the front
of a sample file. The first five seconds of that file are **silence**, so the
short clips contained no speech. Deepgram returned `200 OK` with an empty
transcript and `confidence: 0.000`, quickly, because there was nothing to
decode.

The resulting curve (`≈ 1.26 s + 0.095 s per second`) was a fit to the cost of
uploading silence. It has been discarded.

The lesson is recorded here rather than quietly fixed: **a provider returning
success does not mean it did work.** Any latency measurement must assert that
the response contains the content it was supposed to produce. The harness now
prints the transcript and the word count for exactly this reason.

---

## Translation

Measured through the adapter against `api.inference.net`, steady-state
(excluding the first request), Thai output for English callouts:

| Model | Cold | Warm |
| --- | --- | --- |
| `gemini-3.5-flash-lite` | 2.4 s | **1.05–1.24 s** |
| `gpt-5.6-luna` | 2.6 s | 1.32–1.64 s |
| `gpt-6-luna` | 4.0 s | 1.49–1.81 s |
| `deepseek-v4-flash` | 19.6 s | 1.10–1.81 s |

`gemini-3.5-flash-lite` is the fastest of the four and its Thai output reads
naturally. Note the cold-start spread: `deepseek-v4-flash` took **19.6 seconds**
on its first request, which is long enough that a user would assume the product
had hung.

### Thinking is disabled, and measured

`gpt-6-luna` spends tokens reasoning before answering unless told not to. The
difference on a short callout:

| `reasoning_effort` | Reasoning tokens | First-token latency |
| --- | --- | --- |
| unset (default) | 36 | 304 ms |
| `"none"` | **0** | 209 ms |

The adapter sends `reasoning_effort: "none"` by default and reports the token
count back through `TranslationOutcome::reasoning_tokens`, so "thinking is off"
is a measurement rather than a claim. The example prints it.

### Not every endpoint accepts `reasoning_effort`

Some OpenAI-compatible servers reject unknown parameters outright. Rather than
making this a configuration burden, the adapter detects a `400` whose body names
`reasoning_effort`, retries once without it, and remembers — so the cost is one
failed round trip per process, not one per utterance.

---

## The full chain

```
audio ──► Deepgram ──► transcript ──► translation ──► subtitle
          339 ms                       1050 ms
          ─────────────────────────────────────
          ~1.4 s warm, ~2.4 s typical
```

A callout is usable at 1.4 seconds. It is not *instant*, and the honest
statement is that the product trades a noticeable delay for the ability to
communicate at all.

### Where the remaining latency is

| Term | Cost | Reducible by |
| --- | --- | --- |
| Connection setup | ~1.8 s, **once per session** | Already avoided: pooled connections |
| VAD hangover | 500 ms per utterance | Lowering the hangover, at the cost of splitting sentences |
| Speech-to-text | ~340 ms warm | Streaming (WebSocket) removes the per-utterance fixed cost |
| Translation | ~1.0–1.8 s | A faster model; a smaller prompt |
| **Total** | **~1.4–2.4 s** | |

The two levers worth pulling, in order:

1. **Streaming speech-to-text.** A batch request pays upload and queueing on
   every utterance. A WebSocket session pays it once and emits partial
   transcripts while the speaker is still talking, which is also what lets a
   subtitle appear before the sentence ends.
2. **A smaller translation model.** Translation is now the largest single term.
   `gemini-3.5-flash-lite` was the fastest tested; a purpose-built or
   self-hosted model would likely be faster still.

The VAD hangover is the largest *deliberate* cost. 500 ms is the trade for not
splitting a sentence at a mid-sentence pause, and it is configurable.

---

## Reproducing

```bash
export DEEPGRAM_API_KEY=...
export GAME_BRIDGE_BASE_URL=https://api.inference.net/v1
export GAME_BRIDGE_MODEL=gemini-3.5-flash-lite
export GAME_BRIDGE_API_KEY=...

cargo run -p game-bridge-client --example latency -- --rounds 8 clip.wav
```

Compare auto-detect against a pinned language:

```bash
GAME_BRIDGE_STT_LANGUAGE=en cargo run -p game-bridge-client --example latency -- --rounds 8 clip.wav
```

The probe runs every round in **one process**, deliberately. A shell loop that
invokes the binary repeatedly measures the cold path every time and will report
numbers two to six times worse than what the client experiences.

---

## Caveats

These numbers are from one connection, on one machine, at one time of day. They
are indicative, not a specification:

- Provider queueing dominates variance, and it changes with load.
- No measurement was taken under concurrent load.
- The translation numbers depend on the specific inference.net models tested;
  another provider will differ.
- Speech-to-text was measured on clean, close-miked English speech. Game audio
  is mixed with music, effects, and compressed voice chat, which will be harder
  and is not yet measured.