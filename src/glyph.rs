//! WMO weather codes: their wording, and a glyph for each drawn from prims
//! (circles, arcs, strokes) so the app ships no artwork and every glyph
//! scales with the size it is asked for.

use std::f32::consts::{PI, TAU};

use cce_ui::scene::layout::Rect;
use cce_ui::scene::paint::{Cap, PaintCtx};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sky {
    Clear,
    PartlyCloudy,
    Cloudy,
    Fog,
    Drizzle,
    Rain,
    Snow,
    Thunder,
}

/// WMO 4677 code as Open-Meteo reports it → (sky, words).
pub fn describe(code: u8) -> (Sky, &'static str) {
    match code {
        0 => (Sky::Clear, "Clear"),
        1 => (Sky::Clear, "Mainly clear"),
        2 => (Sky::PartlyCloudy, "Partly cloudy"),
        3 => (Sky::Cloudy, "Overcast"),
        45 => (Sky::Fog, "Fog"),
        48 => (Sky::Fog, "Freezing fog"),
        51 => (Sky::Drizzle, "Light drizzle"),
        53 => (Sky::Drizzle, "Drizzle"),
        55 => (Sky::Drizzle, "Heavy drizzle"),
        56 | 57 => (Sky::Drizzle, "Freezing drizzle"),
        61 => (Sky::Rain, "Light rain"),
        63 => (Sky::Rain, "Rain"),
        65 => (Sky::Rain, "Heavy rain"),
        66 | 67 => (Sky::Rain, "Freezing rain"),
        71 => (Sky::Snow, "Light snow"),
        73 => (Sky::Snow, "Snow"),
        75 => (Sky::Snow, "Heavy snow"),
        77 => (Sky::Snow, "Snow grains"),
        80 => (Sky::Rain, "Light showers"),
        81 => (Sky::Rain, "Showers"),
        82 => (Sky::Rain, "Violent showers"),
        85 | 86 => (Sky::Snow, "Snow showers"),
        95 => (Sky::Thunder, "Thunderstorm"),
        96 | 99 => (Sky::Thunder, "Thunderstorm, hail"),
        _ => (Sky::Cloudy, "Unknown"),
    }
}

// sRGB, as a designer reads them; prim colours are linear, so every use goes
// through `lin`.
const SUN: [f32; 4] = [1.0, 0.72, 0.18, 1.0];
const MOON: [f32; 4] = [0.86, 0.88, 0.95, 1.0];
const CLOUD: [f32; 4] = [0.80, 0.83, 0.88, 1.0];
const CLOUD_DARK: [f32; 4] = [0.52, 0.55, 0.62, 1.0];
const RAIN: [f32; 4] = [0.36, 0.62, 1.0, 1.0];
const SNOW: [f32; 4] = [0.94, 0.96, 1.0, 1.0];
const BOLT: [f32; 4] = [1.0, 0.84, 0.25, 1.0];

fn lin(c: [f32; 4]) -> [f32; 4] {
    cce_ui::color::to_linear(c)
}

/// Draw the glyph for `code` filling the square `rect`. `day` picks sun or
/// moon for the clear and partly-cloudy skies.
pub fn draw(pc: &mut PaintCtx, rect: Rect, code: u8, day: bool) {
    let s = rect.width.min(rect.height);
    let (cx, cy) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
    let (sky, _) = describe(code);
    match sky {
        Sky::Clear => orb(pc, cx, cy, s * 0.5, day),
        Sky::PartlyCloudy => {
            orb(pc, cx + s * 0.14, cy - s * 0.14, s * 0.36, day);
            cloud(pc, cx - s * 0.06, cy + s * 0.10, s * 0.62, CLOUD);
        }
        Sky::Cloudy => {
            cloud(pc, cx + s * 0.12, cy - s * 0.10, s * 0.5, CLOUD_DARK);
            cloud(pc, cx - s * 0.05, cy + s * 0.06, s * 0.72, CLOUD);
        }
        Sky::Fog => {
            cloud(pc, cx, cy - s * 0.12, s * 0.62, CLOUD_DARK);
            let t = (s * 0.06).max(1.5);
            for (i, w) in [0.62f32, 0.48, 0.56].iter().enumerate() {
                let y = cy + s * (0.14 + 0.12 * i as f32);
                let off = if i % 2 == 0 { -s * 0.04 } else { s * 0.05 };
                pc.vector(cx - s * w / 2.0 + off, y, cx + s * w / 2.0 + off, y, t, lin(CLOUD), Cap::Round);
            }
        }
        Sky::Drizzle | Sky::Rain => {
            cloud(pc, cx, cy - s * 0.12, s * 0.72, CLOUD);
            let heavy = sky == Sky::Rain;
            let t = (s * if heavy { 0.065 } else { 0.05 }).max(1.5);
            let len = s * if heavy { 0.18 } else { 0.1 };
            for i in 0..3 {
                let x = cx + s * (-0.18 + 0.18 * i as f32);
                let y = cy + s * 0.18 + if i == 1 { s * 0.06 } else { 0.0 };
                pc.vector(x, y, x - len * 0.4, y + len, t, lin(RAIN), Cap::Round);
            }
        }
        Sky::Snow => {
            cloud(pc, cx, cy - s * 0.12, s * 0.72, CLOUD);
            for i in 0..3 {
                let x = cx + s * (-0.18 + 0.18 * i as f32);
                let y = cy + s * 0.26 + if i == 1 { s * 0.07 } else { 0.0 };
                flake(pc, x, y, s * 0.075);
            }
        }
        Sky::Thunder => {
            cloud(pc, cx, cy - s * 0.14, s * 0.72, CLOUD_DARK);
            let t = (s * 0.06).max(1.5);
            let pts = [
                (cx + s * 0.04, cy + s * 0.06),
                (cx - s * 0.08, cy + s * 0.24),
                (cx + s * 0.04, cy + s * 0.24),
                (cx - s * 0.06, cy + s * 0.44),
            ];
            for w in pts.windows(2) {
                pc.vector(w[0].0, w[0].1, w[1].0, w[1].1, t, lin(BOLT), Cap::Round);
            }
        }
    }
}

/// The sun (disc and rays) or the moon (a crescent) inside a box `d` wide.
fn orb(pc: &mut PaintCtx, cx: f32, cy: f32, d: f32, day: bool) {
    if day {
        let r = d * 0.26;
        pc.circle(cx, cy, r, lin(SUN));
        let t = (d * 0.06).max(1.2);
        for i in 0..8 {
            let a = i as f32 * TAU / 8.0;
            let (sn, cs) = a.sin_cos();
            let (r0, r1) = (r + d * 0.08, d * 0.48);
            pc.vector(cx + cs * r0, cy + sn * r0, cx + cs * r1, cy + sn * r1, t, lin(SUN), Cap::Round);
        }
    } else {
        // A thick arc reads as a crescent and needs no background colour to
        // cut the inner disc away with.
        let r = d * 0.36;
        pc.arc(cx, cy, r, r * 0.55, PI * 0.35, PI * 1.55, lin(MOON));
    }
}

/// A cloud `w` wide centred on (cx, cy): three puffs on a rounded base.
fn cloud(pc: &mut PaintCtx, cx: f32, cy: f32, w: f32, color: [f32; 4]) {
    let color = lin(color);
    let h = w * 0.3;
    let base = Rect { x: cx - w / 2.0, y: cy - h / 2.0 + w * 0.06, width: w, height: h };
    pc.rounded_rect(base, h / 2.0, (true, true, true, true), color);
    pc.circle(cx - w * 0.18, cy, w * 0.2, color);
    pc.circle(cx + w * 0.08, cy - w * 0.08, w * 0.26, color);
    pc.circle(cx + w * 0.3, cy + w * 0.04, w * 0.15, color);
}

/// A six-armed snowflake of radius `r`.
fn flake(pc: &mut PaintCtx, cx: f32, cy: f32, r: f32) {
    let t = (r * 0.35).max(1.2);
    for i in 0..3 {
        let a = i as f32 * PI / 3.0 + PI / 2.0;
        let (sn, cs) = a.sin_cos();
        pc.vector(cx - cs * r, cy - sn * r, cx + cs * r, cy + sn * r, t, lin(SNOW), Cap::Round);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_code_has_words() {
        for code in [0, 1, 2, 3, 45, 48, 51, 53, 55, 56, 57, 61, 63, 65, 66, 67, 71, 73, 75, 77, 80, 81, 82, 85, 86, 95, 96, 99] {
            assert_ne!(describe(code).1, "Unknown", "code {code}");
        }
        assert_eq!(describe(42).1, "Unknown");
    }
}
