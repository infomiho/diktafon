//! The T3-landscape mark: a Rams T3 pocket device lying flat - dial left,
//! 3x3 grille right, magenta hub (see docs/design.md "The mark"). This
//! module is the single source of truth for its geometry: the settings
//! brand row and the menu bar icon draw from these constants, and
//! `diktafon --gen-mark` regenerates the SVG assets (README mark, app icon
//! art) that scripts/build-icon.sh turns into the icns.
//!
//! All numbers live in a 48x48 design box.

use crate::theme;

pub const BODY_X: f32 = 4.;
pub const BODY_Y: f32 = 13.;
pub const BODY_W: f32 = 40.;
pub const BODY_H: f32 = 22.;
pub const BODY_R: f32 = 6.;
pub const DIAL_X: f32 = 14.5;
pub const DIAL_Y: f32 = 24.;
pub const DIAL_R: f32 = 6.6;
pub const HUB_R: f32 = 2.2;
pub const GRILLE_XS: [f32; 3] = [27., 31.5, 36.];
pub const GRILLE_YS: [f32; 3] = [18.5, 24., 29.5];
pub const GRILLE_R: f32 = 1.7;

pub fn grille() -> impl Iterator<Item = (f32, f32)> {
    GRILLE_YS
        .into_iter()
        .flat_map(|y| GRILLE_XS.into_iter().map(move |x| (x, y)))
}

/// The device as one even-odd path (dial and grille punched out), scaled by
/// `s` and translated so the body's top-left lands at (x, y).
fn device_path(x: f32, y: f32, s: f32) -> String {
    let tx = |v: f32| x + (v - BODY_X) * s;
    let ty = |v: f32| y + (v - BODY_Y) * s;
    let rect = {
        let (x0, y0, x1, y1, r) = (tx(BODY_X), ty(BODY_Y), tx(BODY_X + BODY_W), ty(BODY_Y + BODY_H), BODY_R * s);
        format!(
            "M {x} {y0} H {xr} A {r} {r} 0 0 1 {x1} {yr} V {yb} A {r} {r} 0 0 1 {xr} {y1} \
             H {x} A {r} {r} 0 0 1 {x0} {yb} V {yr} A {r} {r} 0 0 1 {x} {y0} Z",
            x = x0 + r,
            xr = x1 - r,
            yr = y0 + r,
            yb = y1 - r,
        )
    };
    let circle = |cx: f32, cy: f32, r: f32| {
        let (cx, cy, r) = (tx(cx), ty(cy), r * s);
        format!(
            "M {x0} {cy} A {r} {r} 0 1 0 {x1} {cy} A {r} {r} 0 1 0 {x0} {cy} Z",
            x0 = cx + r,
            x1 = cx - r,
        )
    };
    let mut d = rect;
    d.push_str(&circle(DIAL_X, DIAL_Y, DIAL_R));
    for (gx, gy) in grille() {
        d.push_str(&circle(gx, gy, GRILLE_R));
    }
    d
}

fn hex(color: u32) -> String {
    format!("#{:06X}", color >> 8)
}

/// The README mark: the device on an indigo tile.
fn tile_svg() -> String {
    let s = 1.2;
    let (x, y) = (8., 18.8);
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
  <rect x="1" y="1" width="62" height="62" rx="16" fill="{bg}"/>
  <rect x="1.5" y="1.5" width="61" height="61" rx="15.5" fill="none" stroke="{hairline}" stroke-opacity=".13"/>
  <path fill-rule="evenodd" fill="{face}" d="{device}"/>
  <circle cx="{hx}" cy="{hy}" r="{hr}" fill="{accent}"/>
</svg>
"##,
        bg = hex(theme::BACKGROUND),
        hairline = hex(theme::HAIRLINE),
        face = hex(theme::TEXT_PRIMARY),
        device = device_path(x, y, s),
        hx = x + (DIAL_X - BODY_X) * s,
        hy = y + (DIAL_Y - BODY_Y) * s,
        hr = HUB_R * s,
        accent = hex(theme::ACCENT),
    )
}

/// The bare mark with transparent holes, for placing on any dark surface.
fn flat_svg() -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{BODY_X} {BODY_Y} {BODY_W} {BODY_H}">
  <path fill-rule="evenodd" fill="{face}" d="{device}"/>
  <circle cx="{DIAL_X}" cy="{DIAL_Y}" r="{HUB_R}" fill="{accent}"/>
</svg>
"##,
        face = hex(theme::TEXT_PRIMARY),
        device = device_path(BODY_X, BODY_Y, 1.),
        accent = hex(theme::SIGNAL_MAGENTA),
    )
}

/// The app icon art in Apple's Big Sur grid (824pt body on a 1024pt canvas):
/// the device on the Signal gradient squircle with an aurora ember foot.
fn app_icon_svg() -> String {
    let s = 14.;
    let (x, y) = (512. - BODY_W / 2. * s, 512. - BODY_H / 2. * s);
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">
  <defs>
    <linearGradient id="ground" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#1B1E3F"/>
      <stop offset="1" stop-color="#0B0D20"/>
    </linearGradient>
    <radialGradient id="ember" cx="0.5" cy="1.02" r="0.55">
      <stop offset="0" stop-color="#FF5A36" stop-opacity="0.26"/>
      <stop offset="1" stop-color="#FF5A36" stop-opacity="0"/>
    </radialGradient>
  </defs>
  <rect x="100" y="100" width="824" height="824" rx="185" fill="url(#ground)"/>
  <rect x="100" y="100" width="824" height="824" rx="185" fill="url(#ember)"/>
  <path fill-rule="evenodd" fill="{face}" d="{device}"/>
  <circle cx="{hx}" cy="{hy}" r="{hr}" fill="{accent}"/>
</svg>
"##,
        face = hex(theme::TEXT_PRIMARY),
        device = device_path(x, y, s),
        hx = x + (DIAL_X - BODY_X) * s,
        hy = y + (DIAL_Y - BODY_Y) * s,
        hr = HUB_R * s,
        accent = hex(theme::ACCENT),
    )
}

/// `diktafon --gen-mark`: regenerate the SVG assets from this geometry.
pub fn write_assets() -> std::io::Result<()> {
    std::fs::create_dir_all("assets")?;
    std::fs::write("assets/diktafon-mark.svg", tile_svg())?;
    std::fs::write("assets/diktafon-mark-flat.svg", flat_svg())?;
    std::fs::write("assets/AppIcon.svg", app_icon_svg())?;
    println!("wrote assets/diktafon-mark.svg, diktafon-mark-flat.svg, AppIcon.svg");
    Ok(())
}
