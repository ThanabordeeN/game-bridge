# Virtual Audio Driver

This directory holds the Windows virtual audio driver that makes the
`🎙 Game Bridge Microphone` device appear to games and Discord (§9).

**Status: not built, not signed, not tested.** What is here is the repository
layout, the component contract, and the decisions that need to be made before
anyone writes a line of driver code.

---

## Why this is a separate deliverable

A Windows audio driver is a kernel-mode component. It cannot be:

- a crate in the client's `Cargo.toml`,
- shipped inside the client's installer as an embedded blob,
- or installed without administrator rights.

It also has to pass Microsoft's driver signing process, which takes days to
weeks. Keeping it in its own directory with its own release cadence means a
client bug fix does not wait on a driver signing round trip.

---

## What the driver must do

```
User-mode client
      │  PCM, 48 kHz
      ▼
┌─────────────────────────┐
│  Virtual Render Endpoint│  ← client writes translated speech here
└───────────┬─────────────┘
            │  kernel-mode bridge
            ▼
┌─────────────────────────┐
│  Virtual Capture Endpoint│  ← games read "Game Bridge Microphone" here
└─────────────────────────┘
```

The two endpoints are one device from the user's perspective. The client writes
to the render side; the game reads from the capture side.

---

## Architecture options

Three approaches are viable, listed in the order worth evaluating.

### 1. APO or virtual audio device via the Windows Audio Device API

Modern, supported on Windows 10 2004+, and what commercial virtual-microphone
products use. Requires a signed driver package but no separate kernel driver
development beyond the audio device.

**Recommended.** It is the path with the least kernel surface and the clearest
signing story.

### 2. SYSVAD-based kernel driver

Microsoft's `sysvad` sample is the canonical starting point for a virtual audio
device. Well documented, but it is a full kernel driver: a bug is a blue screen,
and the review burden is correspondingly high.

Use only if option 1 proves insufficient.

### 3. A user-mode cable and a loopback device (no driver)

Some products ship a user-mode application plus an existing third-party virtual
cable. It avoids signing entirely, but it adds a dependency the user must
install and trust, and it puts a third party's branding in the middle of the
product. Not recommended for a shipped product; useful as a stopgap.

---

## Signing path

| Stage | Signing | User action | Distribution |
| --- | --- | --- | --- |
| Development | Test signing, or none with test mode on | Enable Windows test mode, reboot | Developer only |
| Internal alpha | Test certificate | Install the certificate, reboot | Testers |
| Production | EV certificate + Microsoft attestation signing | None | Everyone |

Development and production must be separate build configurations. A test-signed
driver will not load on a machine with Secure Boot and test mode off, and
discovering that during a release is avoidable.

---

## Layout

```
virtual-audio-driver/
├── README.md            this file
├── docs/
│   ├── architecture.md  endpoint model and buffer strategy
│   └── build.md         build and signing steps
└── driver/
    ├── gamebridge.inf   driver package definition (skeleton)
    └── README.md        what goes here and in what order
```

---

## Interface the client depends on

The client only needs three things from the driver, and they are already
expressed as a trait in
[`client/src/device/virtual_mic.rs`](../client/src/device/virtual_mic.rs):

1. The capture endpoint appears with the friendly name
   **`Game Bridge Microphone`**, and its stable endpoint ID is discoverable.
2. Rendering 48 kHz mono PCM to the render endpoint is audible, latency-bounded,
   and does not block the writer.
3. When the driver is absent, discovery finds nothing — the client must be able
   to tell "not installed" from "installed but broken", because §40 shows
   different messages for each.

Nothing about the client's internal audio path depends on which of the three
architecture options above is chosen.
