---
name: "Signal"
colors:
  background: "#101229"        # window ground; the deep void
  surface: "#171934"           # pill glass, input/card ground
  surface-raised: "#242849"    # hover fills, active nav, popovers
  surface-sunken: "#0B0D20"    # sidebar; deepest step
  on-surface: "#F1F2FF"        # words and labels; cool white
  on-surface-muted: "#AAACD6"  # descriptions, secondary readouts
  outline: "#9B9DFF"           # hairline borders; violet-tinted, at ~13% alpha
  accent: "#9843C0"            # the polishing magenta deepened for fills; buttons, focus ring, links
  accent-hover: "#A853D1"
  accent-active: "#8835AE"
  on-accent: "#F1F2FF"        # white on accent passes AA (4.8:1)
  selection: "#9843C0"         # accent at ~25% alpha
  signal-red: "#FF3B4D"        # recording; doubles as danger
  signal-white: "#FFFFFF"      # transcribing; doubles as success/completion
  signal-magenta: "#CE5CFF"    # polishing; the accent is this hue deepened
  ring-idle: "#8E90BE"         # orbit at rest; muted, never glows
  aurora-ember: "#FF5A36"      # recording wash companion; doubles as warning
  aurora-rose: "#E0459E"       # recording wash companion
typography:
  display:                     # pane titles
    fontFamily: Chakra Petch   # embedded in the binary (SemiBold)
    fontSize: 24px
    fontWeight: 600
    lineHeight: 1.2
  body:                        # control labels, input text
    fontFamily: system
    fontSize: 15px
    fontWeight: 400
    lineHeight: 1.5
  label:                       # descriptions, pill text, secondary lines
    fontFamily: system
    fontSize: 13px
    fontWeight: 400
    lineHeight: 1.4
  mono:                        # technical values: model names, paths
    fontFamily: Menlo
    fontSize: 13px
    fontWeight: 400
    lineHeight: 1.4
  readout:                     # numeric readouts; each glyph in a fixed-width cell
    fontFamily: system
    fontSize: 11px
    fontWeight: 400
    lineHeight: 1.0
rounded:
  sm: 4px
  md: 6px                      # kit controls: inputs, buttons, selects
  lg: 8px                      # nav rows, cards, dialogs
  full: 999px                  # the pill capsule, orbit dots
spacing:                       # 8pt rhythm with 4pt half-steps
  xs: 4px
  sm: 8px
  md: 12px
  lg: 16px
  xl: 24px
  2xl: 32px                    # window padding, section gaps
components:
  control:
    height: 40px               # one height for every control in a window
  button-primary:
    backgroundColor: "{accent}"
    textColor: "{on-accent}"
  button-secondary:
    backgroundColor: "{surface-raised}"
    textColor: "{on-surface}"
    borderColor: "{outline} @ 13%"
  input:
    backgroundColor: "{background}"
    borderColor: "{outline} @ 13%"
    focusRing: "{accent}"
  textarea:                    # multi-line values (the prompt); auto-grows 2-6 rows
    backgroundColor: "{background}"
    borderColor: "{outline} @ 13%"
  switch:
    trackColor: "#5A6086"      # own lighter step; raised surface vanishes at track size
    checkedColor: "{accent}"
  nav-item:
    height: 40px
    rounded: "{rounded.lg}"
    activeBackground: "{surface-raised}"
  settings-window:
    size: 720x500px
    sidebarWidth: 200px
    titlebar: hidden           # transparent full-size content; traffic lights float over the sidebar
    sidebarTopPadding: 48px    # clears the floating traffic lights
    saving: auto-apply         # macOS-style; no Save button, no footer
  keycap:
    height: 28px
    backgroundColor: "{surface-raised}"
    borderColor: "{outline} @ 13%"
    borderBottom: 2px          # reads as a physical key
    rounded: "{rounded.md}"
  status-card:                 # structured state display (daemon); never raw text lines
    backgroundColor: "{surface}"
    borderColor: "{outline} @ 13%"
    rounded: "{rounded.lg}"
    dot: 8px                   # signal-magenta = alive, ring-idle = not; never glows
    rows: muted label left, truncating mono value right
  pill:
    size: 172x38px             # fits content up to 260px for long texts (errors, download)
    rounded: full
    backgroundColor: "{surface} @ 91%"
    borderColor: "{outline} @ 13%"
    meter: the mark's grille extended to 5x3 dots  # red rides the voice; white scan transcribing; magenta twinkle polishing
---

# Signal

Extracted from a cyberpunk audio-visualizer reference (Pixflow "Cyberpunk
Equalizers"): a deep indigo void, neon strokes that mean live signal, cool
white type, thin-line chrome. The translation for a desktop app is
restrained: the desktop is the void, the pill is a dark indigo glass chip,
windows are flat indigo panes, and neon is spent only on elements that
represent the audio signal. Structure of this document follows the Tactile
design-system format (token block, then rules).

Tokens live in `crates/diktafon/src/theme.rs` as `0xRRGGBB00` constants
(alpha appended at the call site); the settings window gets them through a
gpui-component `ThemeConfig` built from the same constants, so the pill and
the windows cannot drift apart.

## Core rule

Glow means live signal. Only the element that carries the signal (the orbit,
the aurora wash) may glow, and its glow intensity rides the signal level.
Chrome, text, and containers never glow.

## Color roles

- Surfaces step by lightness within one indigo hue: sunken sidebar, window
  void, surface, raised. Depth comes from these steps plus hairline outlines,
  never from drop shadows.
- The phase language is fixed: red = recording, white = transcribing,
  magenta = polishing. Status colors derive from it: danger is signal red,
  warning is aurora ember, success/completion is white (Signal has no green;
  a paste completes with a white bloom, and any future "saved / done" state
  is white, not green).
- Magenta is the single interactive accent: primary buttons, focus rings,
  selection, links. It is the polishing hue deepened for large fills
  (white text passes AA on it); both mean "diktafon is acting". The vivid
  SIGNAL_MAGENTA stays reserved for the pill's glowing orbit.
- The palette is designed in OKLCH: every surface step sits on hue 277 with
  restrained chroma (large fills never exceed C 0.06); saturation is spent
  on signal, not on chrome.
- Text is never tinted by phase or accent color: the orbit carries the
  color, text carries the words.

## Elevation & depth

Three levels, no stacking beyond them:

- **Flat** (default): hairline outline at ~13% alpha, no shadow. Windows,
  cards, inputs, the pill chip. Kit shadows are disabled.
- **Raised**: the `surface-raised` fill, still shadowless; hover states,
  active nav, popovers and menus.
- **Glow** (signal only): zero-offset colored `BoxShadow`, blur 4-22px,
  alpha scaled by the live level; at level zero there is no glow.

The pill window disables the system shadow and lets clicks pass through.
A system-blur material for the pill was tried and declined (diktafon-7r4):
over light content the blur washes out the Signal indigo and collapses the
muted text's contrast. The chip stays painted at 91%.

## Type

- Chakra Petch SemiBold for titles only (embedded, so the bundle needs no
  installed fonts); the system font for everything else; Menlo for technical
  values. Steps: display 24/600 (pane titles), body 15/400 (controls), label
  13/400 (descriptions, pill text), mono 13 (model names, paths), readout 11
  (numerics).
- No pane subtitles: the title and the controls say it; anything more is
  fluff.
- Technical values (model names, file paths) render in mono inside
  structured components - a status card with label/value rows - never as raw
  prose that can overflow.
- Numeric readouts render each glyph in a fixed-width cell so changing
  digits never shift the layout.

## Motion

- Signal-driven motion is 1:1 with the audio level: the pill's grille meter
  and the recording aurora both ride fast-attack/slow-decay smoothing with a
  baseline floor, so speech reads as motion and never strobes.
- Ambient motion (processing states) is slow and periodic: a white scan
  sweeping the grille (transcribing), random dots twinkling to a new
  constellation each beat (polishing). Phase changes lerp the grille color
  (~180ms) instead of hard-cutting.
- Microinteractions confirm, never decorate: hover and press feedback
  ~150ms ease-out; state cross-fades 150ms; no staggering of routine
  updates.
- Enter rises in (260ms, strong ease-out); the exit is faster: contents
  fade in place (130ms), then the chip sinks along the entry path (200ms).
  Session endings hold for their beat first: a white wave sweeping the
  grille on paste (420ms), a quiet dim on cancel (160ms), a steady red
  message hold on error (2.4s).
- Under Reduce Motion, ambient and decorative animation freezes to a static
  state; the pill snaps in and out; nothing loses meaning.

## Do's and don'ts

- Spend neon only on the signal; if everything glows, nothing is live.
- Keep chrome thin and flat: hairline outlines, surface steps, no borders
  that exist only to fake elevation.
- One accent. A second saturated hue in chrome dilutes the phase language.
- Keep every control on the shared 40px height inside a window.
- Pair every status color with text; color alone never carries an error.
- Convert relative motion to information: if an animation stops meaning
  something (level, progress, phase), cut it.

## The mark

The T3-landscape: a Rams-style pocket device lying flat - dial left, dot
grille right - an original mark (no third-party license). The single source
of truth is `crates/diktafon/src/mark.rs` (a 48x48 design box: 40x22 body
r6 at (4,13); dial (14.5,24) r6.6, hub r2.2; 3x3 grille dots r1.7 at
x 27/31.5/36, y 18.5/24/29.5). Everything else derives from it:

- `diktafon --gen-mark` regenerates the SVG assets: the README tile
  (`assets/diktafon-mark.svg`), the bare mark
  (`assets/diktafon-mark-flat.svg`), and the app icon art
  (`assets/AppIcon.svg`).
- `scripts/build-icon.sh` renders `AppIcon.svg` into
  `crates/diktafon/resources/diktafon.icns`.
- The settings brand row renders the bare SVG itself via the app's asset
  source (`assets.rs`), so it cannot drift.
- The menu bar (`statusbar.rs`) is the one programmatic rendering, drawn
  from the same `mark` constants because its face is dynamic: the dial hub
  appears while recording, grille dots alternate size while processing.

## Provenance

- Palette and mood: Pixflow "Cyberpunk Equalizers" reference.
- Aurora: the iOS 18+ Siri edge-glow, reinterpreted at pill scale.
- Document structure: the Tactile design system (tokenshelf.dev), structure
  only, no values.
