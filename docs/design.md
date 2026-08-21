# Signal design system

Extracted from a cyberpunk audio-visualizer reference (Pixflow "Cyberpunk
Equalizers"): a deep indigo void, neon strokes that mean live signal, cool
white type, thin-line chrome. The translation for a desktop overlay is
restrained: the desktop is the void, the pill is a dark indigo glass chip, and
neon is spent only on the one element that represents the audio signal.

## Core rule

Glow means live signal. Only the element that carries the signal (the orbit)
may glow, and its glow intensity rides the signal level. Chrome, text, and
containers never glow.

## Palette

Colors are `0xRRGGBB`; alpha is chosen at the call site. Defined in
`crates/diktafon/src/theme.rs`.

| Token | Value | Role |
| --- | --- | --- |
| `SURFACE` | `#14163A` at ~91% | Pill and window ground; deep indigo glass |
| `HAIRLINE` | `#9B9DFF` at ~13% | Borders; violet-tinted, never pure white |
| `TEXT_PRIMARY` | `#F1F2FF` at ~85% | Live words, phase labels; cool white |
| `TEXT_DIM` | `#AAACD6` at ~55% | Secondary readouts (elapsed time) |
| `SIGNAL_RED` | `#FF3B4D` | Recording; the hot neon of the reference |
| `SIGNAL_WHITE` | `#FFFFFF` | Transcribing; the white wave |
| `SIGNAL_MAGENTA` | `#CE5CFF` | Polishing; the magenta bloom |
| `RING_IDLE` | `#8E90BE` | Orbit at rest; muted violet-gray, no glow |

## Depth and glow

- Neon glow is a zero-offset colored `BoxShadow` (blur 4-5px) whose alpha
  scales with the signal level; at level zero there is no glow.
- The pill itself casts no shadow (the system window shadow is disabled); it
  sits on the desktop as a flat glass chip with a hairline border.

## Type

- System font. Cool white, never tinted by the phase color; the orbit carries
  the color, text carries the words.
- Numeric readouts render each digit in a fixed-width cell so changing digits
  never shift the layout.

## Motion

- Signal-driven motion is 1:1 with the audio level (recording orbit), and
  the recording aurora (a Siri-style glow wash: spectrum-fed blobs inside the
  pill plus hue-trading inset edge glow) rides a smoothed level with a
  baseline floor, so it breathes rather than flashes.
- Ambient motion (processing states) is slow and periodic: a soft highlight
  circling at ~0.25 rev/s (transcribing), a radius breath on a ~3s cycle
  (polishing). Nothing strobes; per-frame change stays small.
- The live-transcript marquee crawls at a constant slow speed; it conveys
  that things are happening, not real-time position.
- Enter rises in (200ms), exit sinks out along the same path, softer (300ms).
- Under Reduce Motion, ambient animation freezes to a static state; the pill
  snaps in and out.
