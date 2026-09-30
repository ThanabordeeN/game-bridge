# Binary audio channel

Audio does not travel as JSON. Every WebSocket **binary** message is one audio
frame with a fixed 16-byte header followed by raw PCM.

Base64-in-JSON was rejected: it inflates the stream by ~33% and forces an
encode on the client and a decode on the gateway, adding work to both ends of
the highest-latency-sensitive path in the product.

## Header layout

Little-endian throughout.

| Offset | Size | Field | Value |
| --- | --- | --- | --- |
| 0 | 4 | `magic` | ASCII `GBR1` |
| 4 | 2 | `version` | Protocol version, currently `1` |
| 6 | 1 | `flags` | See below |
| 7 | 1 | `channels` | `1` (mono) or `2` |
| 8 | 4 | `sample_rate_hz` | e.g. `16000` |
| 12 | 4 | `payload_len` | Bytes of PCM that follow |
| 16 | N | `payload` | Signed 16-bit little-endian PCM |

Total message length is `16 + payload_len`. A message whose length disagrees
with `payload_len` is rejected rather than partially processed — a mis-framed
audio stream produces garbage transcripts, and failing loudly is cheaper than
debugging a phantom recognition bug.

## Flags

| Bit | Name | Meaning |
| --- | --- | --- |
| 0 | `FINAL` | Last frame of its segment. |
| 1 | `TRUNCATED` | Segment was cut short, not ended on a natural pause. |
| 2 | `TTS` | Frames flow gateway → client rather than client → gateway. |

Unknown bits are **ignored, not rejected**, so a future version can add flags
without an older client dropping audio. Magic, version, and channel count are
hard failures.

## Direction

| Direction | `TTS` flag | Rate | Content |
| --- | --- | --- | --- |
| client → gateway | clear | 16 kHz mono | Captured speech after VAD |
| gateway → client | set | provider rate, declared per chunk | Translated speech |

Client-to-gateway audio is always 16 kHz mono: that is the native input rate of
every STT provider targeted for v0.1, and 48 kHz would triple bandwidth with no
recognition benefit.

Gateway-to-client audio declares its own rate in the header *and* in the
preceding `tts.audio` event, because TTS providers do not agree on one. The
client resamples to the virtual microphone's rate locally.

## Why 20 ms chunks

20 ms is long enough that header and framing overhead is negligible (16 bytes
per 640) and short enough that VAD onset and streaming STT stay responsive.
Shorter chunks raise syscall and message counts on the hot path; longer chunks
add latency before the gateway can start recognising.

## Frame budget

For a 20 ms chunk at 16 kHz mono 16-bit:

```
payload = 16000 samples/s * 0.020 s * 2 bytes = 640 bytes
message = 640 + 16                           = 656 bytes
bitrate = 656 bytes / 0.020 s                ≈ 262 kbit/s
```

Approximately 32 kB/s upstream while speaking. Because local VAD drops silence
(§7), a user who speaks 25% of the time averages ~8 kB/s.
