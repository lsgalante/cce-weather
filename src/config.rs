//! Where the app's settings come from, and what it remembers.
//!
//! - `~/.config/cce/cce-weather/config.kdl` is the user's declared default:
//!   `units "imperial"` / `units "metric"`, `location "Name" lat=… lon=…`
//!   (optional `region="…"`), `refresh_minutes 15`.
//! - `<state>/cce/weather/state.json` is what the app itself changed — a
//!   searched-for place, the unit toggle — plus the last forecast, so a cold
//!   start shows something before the network answers. Its choices win over
//!   the config's, because they are the more recent decision.

use serde::{Deserialize, Serialize};

use crate::api::Forecast;

const APP: &str = "cce-weather";
pub const DEFAULT_REFRESH_MINUTES: u64 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Units {
    #[default]
    Metric,
    Imperial,
}

impl Units {
    pub fn temp_suffix(self) -> &'static str {
        match self {
            Units::Metric => "°C",
            Units::Imperial => "°F",
        }
    }

    pub fn wind_suffix(self) -> &'static str {
        match self {
            Units::Metric => "km/h",
            Units::Imperial => "mph",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Units::Metric => Units::Imperial,
            Units::Imperial => Units::Metric,
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "metric" | "c" | "celsius" => Some(Units::Metric),
            "imperial" | "f" | "fahrenheit" => Some(Units::Imperial),
            _ => None,
        }
    }

    /// No config, no state: follow the measurement locale. Only the US (and
    /// a couple of others) read Fahrenheit.
    fn from_locale() -> Self {
        let loc = ["LC_ALL", "LC_MEASUREMENT", "LANG"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .find(|v| !v.is_empty())
            .unwrap_or_default();
        let region = loc.split(['.', '@']).next().unwrap_or("").rsplit('_').next().unwrap_or("");
        match region {
            "US" | "LR" | "MM" => Units::Imperial,
            _ => Units::Metric,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub name: String,
    /// "State, Country" as the geocoder gives it; may be empty.
    #[serde(default)]
    pub region: String,
    pub latitude: f64,
    pub longitude: f64,
}

/// Only choices made IN the app are recorded here (a search, the toggle), so
/// an untouched setting keeps following the config when that is edited.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub location: Option<Location>,
    #[serde(default)]
    pub units: Option<Units>,
    #[serde(default)]
    pub forecast: Option<Forecast>,
    /// The place `forecast` is for.
    #[serde(default)]
    pub forecast_location: Option<Location>,
    /// Unix seconds when `forecast` was fetched.
    #[serde(default)]
    pub fetched_at: Option<i64>,
}

pub struct Config {
    pub location: Option<Location>,
    pub units: Option<Units>,
    pub refresh_minutes: u64,
}

pub fn load_config() -> Config {
    let path = cce_ui::config::get_app_config_path(APP);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    parse_config(&text).unwrap_or_else(|e| {
        log::warn!("{}: {e}", path.display());
        parse_config("").unwrap()
    })
}

fn parse_config(text: &str) -> Result<Config, String> {
    let doc: kdl::KdlDocument = text.parse().map_err(|e: kdl::KdlError| e.to_string())?;
    let units = doc
        .get("units")
        .and_then(|n| n.entries().first())
        .and_then(|e| e.value().as_string())
        .and_then(Units::parse);
    let location = doc.get("location").and_then(|n| {
        let num = |k: &str| {
            n.get(k).and_then(|e| e.value().as_f64().or_else(|| e.value().as_i64().map(|i| i as f64)))
        };
        let name = n.entries().iter().find(|e| e.name().is_none())?.value().as_string()?;
        Some(Location {
            name: name.to_string(),
            region: n.get("region").and_then(|e| e.value().as_string()).unwrap_or("").to_string(),
            latitude: num("lat")?,
            longitude: num("lon")?,
        })
    });
    let refresh_minutes = doc
        .get("refresh_minutes")
        .and_then(|n| n.entries().first())
        .and_then(|e| e.value().as_i64())
        .map(|m| m.clamp(5, 24 * 60) as u64)
        .unwrap_or(DEFAULT_REFRESH_MINUTES);
    Ok(Config { location, units, refresh_minutes })
}

fn state_path() -> std::path::PathBuf {
    cce_ui::config::cce_state_dir().join("weather").join("state.json")
}

pub fn load_state() -> State {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Written whole through a temp file and a rename, so a crash mid-write
/// leaves the previous state rather than half a JSON document.
pub fn save_state(state: &State) {
    let path = state_path();
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(state)?)?;
        std::fs::rename(&tmp, &path)
    })();
    if let Err(e) = result {
        log::warn!("saving {}: {e}", path.display());
    }
}

/// The units in force: the app's own toggle, else the config, else the locale.
pub fn resolve_units(state: &State, config: &Config) -> Units {
    state.units.or(config.units).unwrap_or_else(Units::from_locale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let c = parse_config(
            "units \"imperial\"\nlocation \"Brooklyn\" lat=40.65 lon=-73.95 region=\"New York, United States\"\nrefresh_minutes 30\n",
        )
        .unwrap();
        assert_eq!(c.units, Some(Units::Imperial));
        let l = c.location.unwrap();
        assert_eq!(l.name, "Brooklyn");
        assert_eq!(l.region, "New York, United States");
        assert!((l.longitude + 73.95).abs() < 1e-9);
        assert_eq!(c.refresh_minutes, 30);
    }

    #[test]
    fn empty_config_is_all_defaults() {
        let c = parse_config("").unwrap();
        assert!(c.location.is_none() && c.units.is_none());
        assert_eq!(c.refresh_minutes, DEFAULT_REFRESH_MINUTES);
    }

    #[test]
    fn location_needs_both_coordinates() {
        let c = parse_config("location \"Nowhere\" lat=1.0\n").unwrap();
        assert!(c.location.is_none());
    }

    #[test]
    fn integer_coordinates_are_accepted() {
        let c = parse_config("location \"Null Island\" lat=0 lon=0\n").unwrap();
        assert_eq!(c.location.unwrap().latitude, 0.0);
    }
}
