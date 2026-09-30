# Design System

The tokens in [`client/src/ui/theme.rs`](../client/src/ui/theme.rs) and the
widget builders in [`client/src/ui/widgets.rs`](../client/src/ui/widgets.rs),
and the reasoning behind the values (SPEC §18, §33–§37).

No screen hardcodes a colour, radius, size, or spacing value. egui is
immediate-mode with no stylesheet, so consistency across a dozen screens comes
from having exactly one place to change a value and one place to review a diff.

---

## Surfaces (§33)

Layered, not flat. Each level is brighter than the one below it, so a card
visibly sits *above* the background without needing a border.

| Token | Value | Use |
| --- | --- | --- |
| `BASE` | `#0B0B0D` | Window background |
| `SIDEBAR` | `#121215` | Sidebar rail |
| `SURFACE` | `#18181C` | Standard card |
| `SURFACE_RAISED` | `#1F1F24` | Hero card, mini player |
| `SURFACE_HOVER` | `#28282E` | Hover |
| `SURFACE_ACTIVE` | `#303038` | Pressed/selected |
| `DIVIDER` | `#2A2A30` | Hairline |
| `INPUT` | `#0F0F12` | Text fields (recessed) |

**Not pure black.** `#000000` on an OLED panel next to a game's own UI reads as
a hole in the screen rather than a surface. `BASE` is near-black but not black.

A test asserts the ordering is strictly increasing in luminance. If a lower
level were brighter than a higher one, cards would appear to sink into the
background.

---

## Text (§35)

| Token | Value | Use |
| --- | --- | --- |
| `TEXT_PRIMARY` | `#F2F2F4` | Headings, values |
| `TEXT_SECONDARY` | `#A6A6AE` | Labels, body |
| `TEXT_MUTED` | `#6E6E77` | Timestamps, hints, disabled |
| `TEXT_ON_ACCENT` | `#080A0C` | Text on an accent fill |

**Not pure white.** `#FFFFFF` on near-black is harsh for long reading and looks
cheap beside a game's own UI.

---

## Accent (§34)

**`ACCENT` = `#35E0C8`** — a cyan-leaning teal.

SPEC §34 requires Game Bridge to have its own accent, explicitly *not* Spotify
green. Green is the single most recognisable part of Spotify's identity, and
reusing it invites exactly the confusion the spec forbids.

Cyan also works better in the product's actual context: most competitive games
use green for health, party, and "ready" indicators, so a green accent competes
with the game's own HUD. Cyan reads as distinct against both warm HUDs and the
common cool-blue game palettes.

A test measures hue separation from Spotify's `#1DB954`, not RGB distance —
two greens can be far apart in RGB and still read as the same brand colour.

| Token | Use |
| --- | --- |
| `ACCENT` | Start, Live, Connected, Selected, voice activity |
| `ACCENT_DIM` | Pressed accent |
| `ACCENT_WASH` | Selected sidebar item background |
| `WARNING` `#E8A53A` | Low credit, reconnecting |
| `ERROR` `#E05A5A` | Missing driver, connection lost |
| `SUCCESS` `#4CC97A` | Driver installed |

The accent is used **only** for the states SPEC §34 lists. A coloured "Ready"
dot competes with the accent that means "Live", so the idle state is neutral
(`TEXT_MUTED`) rather than coloured.

---

## Type scale (§35)

| Token | Size | SPEC range |
| --- | --- | --- |
| `HERO_SIZE` | 30 | 28–36 |
| `SECTION_SIZE` | 21 | 20–24 |
| `CARD_TITLE_SIZE` | 16 | 15–18 |
| `BODY_SIZE` | 14 | 13–15 |
| `META_SIZE` | 12 | 11–13 |
| `LANGUAGE_SIZE` | 22 | (the hero pair) |

A test asserts each value is inside its SPEC range and that the scale is
strictly decreasing — a scale where "card title" is larger than "section" makes
the hierarchy unreadable regardless of the individual values.

---

## Radii (§36)

| Token | Value | SPEC range |
| --- | --- | --- |
| `RADIUS_CARD` | 14 | 12–16 |
| `RADIUS_SMALL` | 10 | 8–12 |
| `RADIUS_INPUT` | 8 | 8 |
| `RADIUS_PILL` | `u8::MAX` | pill |

Pill buttons use `u8::MAX` rather than a measured half-height: egui clamps the
corner radius per axis, which is how a pill is expressed in that API.

---

## Spacing

A 4-point scale: `XS` 4, `SM` 8, `MD` 16, `LG` 24, `XL` 32. Every gap is a
multiple of the base unit, asserted by a test.

**`SIDEBAR_WIDTH` = 216**, fixed so the content area does not shift between
screens. A test asserts the sidebar plus a 400px content column fits inside
`MIN_WINDOW_SIZE`.

---

## Layout (§19, §20, §30)

```
┌──────────────────────────────────────────────────┐
│ toasts (transient, top)                          │
├──────────────┬───────────────────────────────────┤
│ GAME BRIDGE  │                                   │
│ ● Home       │   content area (scrollable)       │
│ Translation  │                                   │
│ Voice        │                                   │
│ Audio        │                                   │
│ ──────────   │                                   │
│ Usage        │                                   │
│ Billing      │                                   │
│ Settings     │                                   │
│              │                                   │
│ ● Live       │                                   │
│ Thanabordee  │                                   │
│ ฿84.50       │                                   │
├──────────────┴───────────────────────────────────┤
│ ● LIVE  Valorant  TH→EN  08:42   [Bypass][Stop]  │
└──────────────────────────────────────────────────┘
```

The sidebar footer is built bottom-up so the status and user block stay pinned
regardless of how many nav items exist.

The mini player only renders while a session exists. SPEC §30 calls it
persistent, but on an idle Home screen it would be an empty bar.

---

## Components

Built in `widgets.rs` so every instance is identical:

| Builder | Use |
| --- | --- |
| `card` / `raised_card` / `outlined_card` | Content containers |
| `clickable_outlined_card` | Routing mode cards (§27) |
| `primary_button` | "START GAME BRIDGE" |
| `danger_button` | "STOP BRIDGE" |
| `secondary_button` / `small_button` | Inline actions |
| `status_pill` / `dot` | Status indication |
| `section_heading` / `field_label` / `meta_label` | Type roles |
| `labelled_value` | "Input  HyperX QuadCast" |
| `segmented` | `[Subtitle] [Voice Out] [Full Voice]` |
| `waveform` | Voice activity (§22) |
| `readonly_field` | Device summary |

**Stop is filled with `ERROR`, not the accent.** Stopping is not the same kind
of action as starting; using the accent for both would make the live screen's
primary button read as "go" when it means "stop".

**The waveform is capped at 28px tall.** SPEC §22 asks for something subtle that
shows the microphone is working, explicitly not a spectrum analyzer. A test
enforces the cap so a later change cannot quietly turn it into one. It also
draws a 1px floor per bar, because a strip of nothing looks like a broken widget
rather than a quiet microphone.

---

## Interaction (§37)

Few dialogs. Settings use toggles, dropdowns, sliders, and card selection.

- Mode selection is a segmented control, per §37.
- Routing modes are cards, per §27, and the whole card is the click target —
  a card that responds only to a click on its text is a common immediate-mode
  mistake.
- Errors are inline cards with a named cause and an action, per §40, not modals.
- The overlay settings sliders clamp to the same ranges `OverlayStyle::sanitize`
  enforces, so a hand-edited config cannot produce a state the UI cannot
  represent.

---

## Accessibility and legibility

Not a formal audit; these are the properties the tokens were chosen to satisfy:

- `TEXT_PRIMARY` is more than 3× the luminance of the lightest surface it sits
  on, asserted by a test.
- Error and warning are distinguishable by hue (red vs amber) as well as
  luminance, so they survive a colour-vision deficiency where both would appear
  as similar greys.
- The overlay honours `text_outline` for legibility against a bright skybox,
  and its background opacity is independent of text opacity — the text never
  fades.
- Bilingual subtitles keep the original above the translation consistently, so
  the position alone tells the reader which language they are reading.

---

## What is not covered

- **No screenshot tests.** Widget construction compiles and the pure helpers are
  tested, but nothing asserts what the rendered window looks like. A visual
  regression would not be caught.
- **No font selection.** egui's bundled default fonts are used. Thai text
  renders through the system fallback; a shipped build should bundle a font with
  full Thai and Arabic coverage, since the overlay position depends on correct
  metrics.
- **No high-contrast or reduced-motion mode.**
