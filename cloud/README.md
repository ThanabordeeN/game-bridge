# Game Bridge Cloud

This directory contains **documentation only**. The cloud service is proprietary
and lives in a private repository (§17).

---

## Why the cloud exists at all

The client is open source. If it called Deepgram, DeepSeek, Cartesia and
MiniMax directly, it would have to ship their API keys, and an API key in a
published open-source client is a key that is compromised the day it ships.

So the client holds exactly one credential — a short-lived session token — and
the gateway holds everything else.

```
Game Bridge Client
        │  session token (short-lived, revocable, scoped)
        ▼
Game Bridge Gateway
        │
        ├── Deepgram      STT
        ├── DeepSeek      translation
        ├── Cartesia      TTS
        └── MiniMax       premium TTS
```

---

## Service topology (§43)

```
API Gateway
├── Authentication
├── Session Management
├── WebSocket Gateway
│
├── STT Service           → Deepgram
├── Translation Service   → DeepSeek
├── TTS Service           → Cartesia, MiniMax
│
├── Credit Service
├── Usage Metering
├── Rate Limiting
└── Observability
```

---

## What is private, and why

| Component | Why it is not open source |
| --- | --- |
| Provider credentials | The whole point. Shipping them leaks them. |
| Billing and wallet | A client-editable price is not a price. |
| Provider routing rules | Commercial terms with each provider; also the main lever for cost control. |
| Fraud prevention | Published detection rules are defeated detection rules. |
| Rate limiting | Same. |
| Usage metering | The authoritative source of what a user is charged; must not be client-editable. |

---

## The metering contract the client depends on

The client's billing logic is a *presentation* of the gateway's numbers, never
their source. Three rules make that true, and the gateway must hold up its end:

1. **`usage.update` replaces, never merges.** The client overwrites its local
   counters with whatever the gateway sends. A client that could accumulate its
   own totals could be patched to accumulate less.

2. **`credit.update` is authoritative.** The client displays the balance it is
   told. It never adds or subtracts credits by itself.

3. **Rates come from `session.started`.** The client has launch-default rates
   for display before a session begins, but the gateway's table supersedes them
   for the session's whole duration.

The client-side counterparts are in
[`protocol/src/usage.rs`](../protocol/src/usage.rs) (the rate and cost math) and
[`client/src/session/credits.rs`](../client/src/session/credits.rs) (the wallet).

---

## Billing unit: active voice, not wall clock

The product promise (§29) is that silence is not charged. The gateway bills
`speech_ms`, reported by the client's local VAD.

This creates an obvious attack: a patched client under-reports `speech_ms` and
pays less. The gateway cannot verify speech duration independently without
receiving the audio, and receiving all the audio is exactly what local VAD exists
to avoid.

Mitigations available to the gateway, in rough order of cost:

- **Cross-check against STT output.** A session claiming 500 ms of speech but
  returning 40 seconds of transcript is inconsistent; flag it.
- **Bound by wall clock.** Billable speech cannot exceed session duration.
- **Statistical anomaly detection.** A client whose reported speech fraction is
  far outside the population is either broken or modified.
- **Reputation.** Persistent under-reporting from one account is a fraud signal
  on its own.

Perfect enforcement is not achievable, and pretending otherwise is worse than
accepting the tradeoff explicitly: the cost of over-charging honest users is
higher than the cost of a minority of modified clients under-reporting.

---

## Not documented here

Implementation detail of the private service is deliberately absent from this
public repository — endpoint shapes beyond the wire protocol, deployment
topology, provider account structure, and pricing strategy. The client-facing
contract is fully specified in
[`documentation/PROTOCOL.md`](../documentation/PROTOCOL.md).
