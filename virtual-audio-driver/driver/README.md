# Driver source

Empty on purpose.

There is no driver source in this repository yet. See
[`../docs/build.md`](../docs/build.md) for the order of work, which begins by
getting Microsoft's unmodified `sysvad` sample building and installing on a test
VM.

## Why nothing is here

The checkout this scaffold was authored on is Linux. It has no Windows Driver
Kit, no MSBuild, no test-signing certificate, and no machine that could load a
kernel driver to verify anything.

Writing plausible-looking driver source that has never been compiled would be
actively harmful:

- A kernel-mode bug is a **blue screen on a user's machine**, not a caught
  exception.
- Audio callbacks run at raised IRQL; a paging fault there is not recoverable.
- An INF with a wrong hardware ID or format table can mis-bind to unrelated
  hardware.

An honest empty directory plus a build plan is worth more than a thousand lines
of unverifiable code.

## What goes here, in order

1. `gamebridge.sys` — the driver binary, built from a `sysvad`-derived project.
2. `gamebridge.inf` — the package definition. A skeleton
   ([`gamebridge.inf`](gamebridge.inf)) documents the device identity and
   endpoint names the client depends on.
3. `gamebridge.cat` — the catalog, produced at signing time, never by hand.
4. `*.vcxproj` — the MSBuild project derived from the WDK sample.

## The one thing to get right first

The friendly name. `client/src/device/detection.rs` matches the virtual endpoint
on the string `Game Bridge Microphone`. If the driver publishes a different name,
the client reports the driver as missing while it is installed and working — one
of the more confusing failures to debug from the outside.

Keep that string in one place on the driver side too, and make the acceptance
test for step 2 in the build plan assert it.
