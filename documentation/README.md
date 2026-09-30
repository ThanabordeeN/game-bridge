# Documentation

| Document | What it covers |
| --- | --- |
| [`SPEC.md`](SPEC.md) | The product specification, reproduced in full |
| [`INCOMING.md`](INCOMING.md) | The incoming path: audio → text → translate → subtitle |
| [`LATENCY.md`](LATENCY.md) | Measured provider latency, and what drives it |
| [`VAD.md`](VAD.md) | Voice activity detection: the bug real audio found, window tuning, and its limits |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | How the spec's boxes map to code, threading, latency budget, feedback protection |
| [`PROTOCOL.md`](PROTOCOL.md) | The wire contract: every event, the binary frame, reconnection |
| [`DESIGN.md`](DESIGN.md) | Design tokens and the reasoning behind each value |
| [`MVP.md`](MVP.md) | What exists, what does not, and the build order |

Related, outside this directory:

| Document | Location |
| --- | --- |
| Repository overview and capability matrix | [`../README.md`](../README.md) |
| Protocol schemas and golden payloads | [`../protocol/schemas/`](../protocol/schemas/) |
| Virtual audio driver plan | [`../virtual-audio-driver/README.md`](../virtual-audio-driver/README.md) |
| Cloud service and the metering contract | [`../cloud/README.md`](../cloud/README.md) |

---

## Start here

- **What is this product?** → `SPEC.md`
- **What actually works today, and how do I run it?** → `INCOMING.md`
- **Why is the code laid out this way?** → `ARCHITECTURE.md`
- **How do client and gateway talk?** → `PROTOCOL.md`
- **Why does it look like that?** → `DESIGN.md`
- **What actually works today?** → `MVP.md`, and the capability matrix in the
  top-level README

---

## A note on what these documents claim

Every performance figure and status claim in these documents is either:

- **measured on this checkout**, and says so with a test count or a command, or
- **labelled as a target or a plan**, and says so explicitly.

The latency budget in `ARCHITECTURE.md` is a design target, not a measurement.
The driver and cloud documents describe work that has not been done. That
distinction is maintained deliberately: a document that implies unverified code
works is worse than one that admits it does not, because the reader cannot tell
the difference until it fails in front of a user.
