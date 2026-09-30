# Protocol schemas and golden examples

The authoritative definitions are the Rust types in `../src/`. These files exist
so that non-Rust consumers — the gateway's provider adapters, test harnesses,
and anyone writing a third-party client — have a language-neutral reference.

## Files

| File | Purpose |
| --- | --- |
| `events.schema.json` | JSON Schema for the text-channel event envelope and every documented variant. |
| `audio-frame.md` | Byte-level layout of the binary audio channel. |
| `examples/*.json` | Golden payloads, one per event type. |

## Golden examples are load-bearing

`../tests/examples.rs` decodes every file in `examples/` and asserts it matches
the expected variant, then re-encodes it and asserts a structural round-trip.
A rename that breaks the wire contract therefore fails `cargo test`, not a
production deploy.

Add an example whenever you add an event:

```json
{"type":"session.stop"}
```

## Generation policy

`events.schema.json` is **maintained by hand and checked against the Rust
types** by the example tests. It is not generated, because a generated schema
would silently follow any Rust change — including a breaking one — whereas a
hand-maintained schema plus golden examples turns that change into a test
failure that a reviewer must consciously accept.

When you change a wire-visible field:

1. Update `../src/`.
2. Update `events.schema.json`.
3. Update or add an example in `examples/`.
4. Run `cargo test -p game-bridge-protocol`.
5. Note the change in `../../documentation/PROTOCOL.md` under change history.
