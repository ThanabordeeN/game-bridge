Kernel-mode Windows audio driver development notes.

**This file describes work that has not been done.** There is no driver source
in this repository yet: the checkout it was scaffolded on is Linux, with no
Windows Driver Kit, no test-signing certificate, and no machine able to load a
kernel driver. Writing driver source that has never been compiled would be
worse than useless — a kernel-mode bug is a blue screen on a user's machine.

What follows is the order of work and the decisions to make at each step.

---

## Prerequisites

| Requirement | Notes |
| --- | --- |
| Windows 10/11 x64 dev machine | Cannot be cross-compiled from Linux in any practical sense |
| Visual Studio 2022 with C++ workload | The WDK integrates with MSBuild |
| Windows Driver Kit (WDK) | Version must match the SDK |
| Test-signing certificate | `MakeCert` is deprecated; use `New-SelfSignedCertificate` |
| Test machine or VM | Never develop a kernel driver on a daily-driver machine |
| Secure Boot disabled, test mode on | On the test machine only |

---

## Build and load loop

1. Build the driver package with `msbuild /p:Configuration=Debug /p:Platform=x64`.
2. Sign the `.sys` with the test certificate.
3. Enable test mode on the test VM: `bcdedit /set testsigning on`, then reboot.
4. Install: `pnputil /add-driver gamebridge.inf /install`.
5. Confirm the endpoint appears with the friendly name `Game Bridge Microphone`.
6. Render a test tone to it and capture from the other side; measure latency and
   confirm no dropout.

Step 6 is the acceptance test. Steps 1–5 are plumbing.

---

## Order of work

Do these in sequence; each is verifiable before the next depends on it.

1. **Get `sysvad` building and installing unmodified.** Do not write any custom
   code first. A known-good virtual audio device that appears in Device Manager
   is the foundation; debugging a custom driver and a misconfigured build
   environment at the same time is not tractable.
2. **Rename the endpoints** to `Game Bridge Microphone` and confirm the name
   survives a reinstall.
3. **Implement the render-to-capture bridge.** Start with a fixed-size ring and
   no resampling. Confirm a tone written to render appears at capture.
4. **Bound the latency.** Measure the round trip. The client's end-to-end budget
   is tight, and the driver must not be the dominant term.
5. **Handle format negotiation.** The endpoint should accept 48 kHz mono 16-bit,
   which the client resamples to, and reject or convert anything else loudly.
6. **Test the absence case.** Uninstall the driver and confirm the client shows
   the §40 "Virtual Microphone Not Found" state rather than crashing.

---

## Buffer strategy

The driver's internal buffer must be bounded and small. A large buffer is
tempting because it hides scheduling jitter, but it adds latency directly to the
user's translated voice, and latency is the product's central constraint.

Start with a 20 ms buffer and grow only if dropouts appear under load. Document
whatever the measurement turns out to be in `docs/architecture.md`.

---

## What must not happen

- **No echo path.** The capture endpoint must never carry audio written to the
  render endpoint *and* audio captured from the render endpoint's own device.
  That is the driver-level version of the feedback loop the client guards
  against in §13.
- **No unbounded allocation in the audio path.** Kernel audio callbacks run at
  a raised IRQL and cannot page.
- **No writing to user-accessible debug logs in the hot path.** It is a latency
  trap and a potential information leak.

---

## Signing

Development uses test signing. Production needs an EV code-signing certificate
and Microsoft attestation signing.

Do not plan to ship a test-signed driver. It will not load on a user's machine
with Secure Boot enabled, which is the default on every modern Windows install.
