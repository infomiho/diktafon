# Design-system color audit (2026-09-23)

Read-only audit of `docs/design.md` against `crates/diktafon/src/theme.rs`, raw colors in the gpui code, the mock `:root` blocks, and `web/static/style.css`, validated with OKLCH, WCAG and APCA.

## Summary

Every token hex value matches across design.md, theme.rs, the mocks and the web CSS. The drift is in usage rules, undocumented tokens, and contrast.

## Drift

1. Green is used beyond its documented role: the History diff highlight (settings.rs, settings.js) uses SIGNAL_GREEN at 22%, while design.md allows green for permission dots only. theme.rs:30 says "Signal has no green" above the SIGNAL_GREEN constant.
2. Pill status text differs everywhere: design.md says muted, the app uses faint (pill.rs:635), the mock uses muted at 55% (pill.js:77, about 3.3:1).
3. Links differ: the app uses ACCENT (#9843C0) as link text, the web uses #CE5CFF.
4. Accent icon hover: the mock uses accent-hover (dk.js:119), the app keeps ACCENT (settings.rs:1929).
5. Stale comment at settings.rs:1575 ("White = alive", the code uses magenta).
6. The mark hub is ACCENT in the app icon and README tile but SIGNAL_MAGENTA in the flat mark (mark.rs). Undocumented.
7. Aurora ember has three jobs: the recording aurora, the kit warning with the warning notice text, and the app icon tint.
8. Raised (C 0.060) and RAISED_HOVER (C 0.061) sit on the documented "large fills never exceed C 0.06" limit.

Undocumented in design.md: raised-hover #2B2F51, raised-active #1D213F, outline alphas 0x14, 0x44 and 0x50, overlay sunken at 50%, icon gradient stop #1B1E3F (raw literal in mark.rs), danger and warning foreground literals in theme.rs (unused by any rendered kit component).

## Contrast failures (WCAG)

| Pair | Ratio | Where |
|---|---|---|
| faint #7F84A2 on raised | 3.88 | History timestamps on hovered or selected rows |
| faint on the pill over a white desktop | 3.61 | pill status and time |
| on-accent on accent-hover #A853D1 | 3.92 | every primary button on hover |
| accent #9843C0 as text on surface / raised | 3.18 / 2.64 | Third-party notices link, accent icons |
| focus ring (accent) on raised | 2.64 | focus on raised elements |
| switch track #5A6086 on surface | 2.82 | only if a switch sits on a card |

Warning text in ember passes contrast (5.52:1 on surface) but fails meaning: ember and signal-red are 12° apart in hue at nearly the same lightness. OKLab distance is 0.057 for normal vision, 0.036 for deutan (both simulate to olive) and 0.021 for tritan. Red-orange reads as error or recording, not warning.

## Proposed values (checked for sRGB gamut and contrast)

| Token | Value | Why |
|---|---|---|
| warning (new, signal-amber) | oklch(0.83 0.155 80) = #FCBB39 | 10.1:1 on surface, 8.3 on raised. Distance from signal-red: 0.263 normal, 0.175 deutan |
| accent-text (new) | oklch(0.70 0.18 313) = #C578ED | 6.4:1 on background, 5.0 on raised, for links and accent icons |
| accent-hover | oklch(0.52 0.19 313) = #8E3CB4 | darken on hover, 5.47:1 with on-accent. accent-active about oklch 0.48 |
| on-surface-faint | oklch(0.66 0.046 277.6) = #8B90AF | 4.55 on raised, 4.23 on the pill over white |
| surface-sunken (optional) | about oklch(0.150 0.036 277) | sunken vs background is only 1.04:1 today |
| switch track (optional) | about oklch(0.53 0.06 277) | if switches ever sit on cards |

## Recommendations

High:
1. Add the amber warning token, point theme `colors.warning` and `warning_foreground` at it, update design.md "warning is aurora ember" and the mock `--warning`. Ember keeps only the aurora and icon roles.
2. Add `accent-text` for links and accent icons (app, mock, dk.js).
3. Darken primary button hover and active instead of lightening.

Medium:
4. Raise faint to #8B90AF everywhere, including web `--text-faint`.
5. Settle the pill status color on faint and update design.md and pill.js.
6. Allow the diff highlight in the green rule, or switch it to an accent tint. Remove the "Signal has no green" comment.
7. Document the undocumented tokens and make the icon gradient stop a constant.

Low:
8. Switch track contrast. 9. Widen the sunken step. 10. Fix the stale comment, align the mock accent hover, document the mark hub rule. 11. Consolidate the hover recipes into one or two tokens.

## Not verified

Rendered pixels were not screenshotted. Pill-over-desktop numbers assume worst-case solid black or white behind the 91% chip. Colorblind numbers are simulations (Machado 2009). APCA uses the 0.0.98G formula.
