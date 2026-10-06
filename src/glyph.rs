//! WMO weather codes: their wording, and the glyph for each — one of the
//! multicolour `weather-*` glyphs of the cce-icons set, drawn through
//! cce-ui's `PaintCtx::icon_untinted`.

use cce_ui::scene::layout::Rect;
use cce_ui::scene::paint::PaintCtx;

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

/// The cce-icons glyph for `sky`. `day` picks sun or moon for the clear and
/// partly-cloudy skies; the rest have one glyph for day and night alike.
pub fn icon_name(sky: Sky, day: bool) -> &'static str {
    match (sky, day) {
        (Sky::Clear, true) => "weather-clear-day",
        (Sky::Clear, false) => "weather-clear-night",
        (Sky::PartlyCloudy, true) => "weather-partly-cloudy-day",
        (Sky::PartlyCloudy, false) => "weather-partly-cloudy-night",
        (Sky::Cloudy, _) => "weather-cloudy",
        (Sky::Fog, _) => "weather-fog",
        (Sky::Drizzle, _) => "weather-drizzle",
        (Sky::Rain, _) => "weather-rain",
        (Sky::Snow, _) => "weather-snow",
        (Sky::Thunder, _) => "weather-thunder",
    }
}

/// Draw the glyph for `code` filling the square `rect`. The weather glyphs
/// carry their own colours, so they are drawn untinted.
pub fn draw(pc: &mut PaintCtx, rect: Rect, code: u8, day: bool) {
    let (sky, _) = describe(code);
    pc.icon_untinted(icon_name(sky, day), rect, 1.0);
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

    /// Every glyph named is one the cce-icons set ships.
    #[test]
    fn every_sky_names_a_shipped_glyph() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../cce-icons/svg");
        if !dir.is_dir() {
            eprintln!("skipping: no cce-icons checkout beside this crate");
            return;
        }
        let skies = [Sky::Clear, Sky::PartlyCloudy, Sky::Cloudy, Sky::Fog, Sky::Drizzle, Sky::Rain, Sky::Snow, Sky::Thunder];
        for sky in skies {
            for day in [true, false] {
                let name = icon_name(sky, day);
                assert!(dir.join(format!("{name}.svg")).is_file(), "{sky:?} day={day}: no {name}.svg");
            }
        }
        assert_ne!(icon_name(Sky::Clear, true), icon_name(Sky::Clear, false));
        assert_ne!(icon_name(Sky::PartlyCloudy, true), icon_name(Sky::PartlyCloudy, false));
    }
}
