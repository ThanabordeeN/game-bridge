# Voice Activity Detection

What the VAD does, what it was measured against, the bug that real audio found,
and the limitation that remains.

The measurement harness is `cargo run -p game-bridge-client --example segments`.
Everything below is reproduced from real runs, not estimated.

---

## Why the VAD is load-bearing

The VAD is what makes SPEC §29's promise true — *"you are charged only while
speech is detected; silence is not charged"*. It is also the only thing standing
between the user's bandwidth and the cloud: whatever it passes is uploaded,
transcribed, and billed.

That means both failure directions are expensive and they look identical from a
passing unit test:

| Failure | Consequence |
| --- | --- |
| Passes too much | Silence and music are uploaded and **billed** |
| Passes too little | Words are clipped, and the subtitle is wrong |

---

## How it works

Per 10 ms frame: RMS energy in dBFS, plus a zero-crossing rate. A frame is
speech if it clears an absolute threshold, is not broadband noise, and stands a
margin above the noise floor.

The noise floor is the **quietest frame in a sliding window** — not a smoothed
average. That distinction is the fix for the bug below.

```
absolute gate   energy >= -45 dBFS          rejects digital silence outright
transient gate  zcr    <= 0.30              rejects clicks, clacks, room hiss
floor gate      energy >= floor + 9 dB      rejects a background bed
```

---

## The bug real audio found

The floor used to be seeded from the first frame classified as *not* speech, and
updated only on such frames. That is a **lock**:

> If a recording starts with speech — or with any sound above the absolute
> threshold — no frame is ever "not speech", so the floor never seeds, so the
> margin check never engages, so every low-ZCR sound above the threshold is
> classified as speech.

Every unit test missed it, because every unit test fed quiet frames first. The
first run against a real recording reported **"dropped silence: 0.00 s"** — the
detector calling an entire file speech.

Isolated with a constructed clip — 5 s speech, 3 s of an 80 Hz tone at −40 dBFS,
3 s speech — the tone has a low zero-crossing rate and sits above the absolute
threshold, so only the floor could reject it:

| | Segments | Speech | Longest segment |
| --- | --- | --- | --- |
| Before | 3 | 9.21 s | **4.69 s** |
| After | 3 | 8.23 s | 3.69 s |

The 4.69-second segment spanned the tone region: a music bed absorbed into a
"callout", sent to the cloud, and billed.

In a real game this is the normal case, not an edge case — there is always a
music and effects bed under the voice chat.

### The fix, and the second bug it exposed

The floor became the minimum over a sliding window, which cannot lock up. That
immediately broke every synthetic test, and the reason is worth recording: the
tests used a **constant-amplitude tone** as "speech".

A steady tone is genuinely indistinguishable from a steady noise floor by energy
alone — the floor equals the signal, so `energy >= floor + margin` can never
hold. The test signal was unrealistic, not the fix. It was replaced with a
syllabic envelope, because the property energy detection actually relies on is
that speech has a large dynamic range: over a short window, its quietest moment
is far below its peaks.

A second bootstrap problem then appeared: with a pure sliding minimum, the
**first frame's floor equals itself**. The floor therefore needs a warm-up
before it is trusted; until 200 ms of audio has been seen, only the absolute
threshold applies.

---

## Choosing the window

The floor cannot learn a background bed faster than one window, so **the window
*is* the adaptation lag**. Measured on the constructed tone-bed clip:

| Window | Tone leakage into speech | Real clip: segments | Real clip: speech |
| --- | --- | --- | --- |
| 2000 ms | 2.46 s | 6 | 81% |
| 1000 ms | 1.46 s | 6 | 75% |
| **500 ms** | **0.96 s** | **6** | **74%** |
| 300 ms | 0.76 s | 6 | 73% |

The obvious risk of a short window — the floor rising during continuous speech
until nothing clears the margin — **did not appear**. A real 14.5-second
narration clip produced the same six segments at every window from 2 s down to
300 ms.

500 ms is the default: roughly two to three syllables, long enough to contain a
genuinely quiet moment and short enough to follow a changing bed. 300 ms
measured only marginally better and leaves less margin.

---

## What the detector cannot do

Against a real film clip with a music intro, the VAD segmented the intro as
speech. Transcribing each segment showed what was actually there:

```
00   1181 ms  skipped (nothing translatable, confidence 0.00)   <- music
01    350 ms  skipped (nothing translatable, confidence 0.00)   <- music
02    556 ms  [German] Jap.                                     <- fragment
03    509 ms  [English] I said it before, and I'll say it again.
04    349 ms  [English] Life moves pretty fast.
05    267 ms  [English] You don't stop and look around once in a while.
```

Segments 00 and 01 were called speech by the VAD and produced nothing when
transcribed.

**This is a fundamental limit, not a tuning failure.** Music has energy
dynamics like speech and a low zero-crossing rate like voiced speech. Energy
plus ZCR cannot separate them; that needs spectral features or a model.

### Why it still matters, and the design consequence

The *pipeline* handles it correctly: Deepgram returns an empty transcript,
`is_translatable` rejects it, and nothing reaches translation or the subtitle.
No garbage is shown to the user.

The **billing** does not. Active voice is currently measured by the VAD, so a
music intro counts as billable speech the user never spoke.

That is a real problem for SPEC §29, and it is worth stating rather than
smoothing over. Three options, in order of preference:

1. **Measure active voice where the truth is available.** Count a segment as
   billable only once speech-to-text returns a non-empty transcript. The VAD
   stays a cheap gate for *upload*, and billing follows what was actually
   transcribed. This is more accurate, more defensible to a user, and costs
   nothing extra — the transcript is already coming back.
2. **Add a spectral VAD.** Zero-crossing rate replaced by spectral flatness or a
   small model. Removes the misclassification at the source, and the `Vad` seam
   already allows it. More work, and more dependency.
3. **Accept it and document it.** Weakest: it charges users for their game's
   soundtrack, which is exactly the complaint the pricing model was designed to
   avoid.

Option 1 is recommended and is a change to the session accounting, not to the
detector.

---

## Reproducing

```bash
# segment a file and report the energy profile
cargo run -p game-bridge-client --example segments -- --verbose clip.wav

# transcribe each segment, to see what the detector actually passed
DEEPGRAM_API_KEY=... cargo run -p game-bridge-client --example segments -- --stt clip.wav

# sweep a parameter without recompiling
VAD_FLOOR_WINDOW_MS=500 cargo run -p game-bridge-client --example segments -- clip.wav
```

Tunable from the environment: `VAD_FLOOR_WINDOW_MS`, `VAD_FLOOR_TRUST_MS`,
`VAD_HANGOVER_MS`, `VAD_MARGIN_DB`.

The harness prints the **energy percentiles and the fraction of frames above
the absolute threshold** before the segmentation, because that is what explains
the detector's behaviour. A detector that calls everything speech is usually a
detector whose floor found no quiet to learn from, and the percentiles show
whether any quiet existed.

---

## Still unmeasured

- **Real game audio.** Every test above uses clean speech, or synthetic beds.
  Game audio mixes compressed voice chat, music, effects, and gunfire, and none
  of that has been measured.
- **The hangover against real conversation.** 500 ms is still chosen from
  reasoning. Whether it splits real callouts needs recorded team comms.
- **Non-English speech.** All the real audio used here is English. Thai has
  different energy dynamics and no inter-word pauses, which may need a different
  margin.