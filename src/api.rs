//! Open-Meteo: the forecast and the geocoder. Both are keyless HTTPS JSON
//! endpoints, called from worker threads (blocking reqwest) — never from the
//! frame loop.

use serde::{Deserialize, Serialize};

use crate::config::{Location, Units};

const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";
const GEOCODE_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// The fields asked for, per block. Each name is also a key of the response
/// struct below, so the two lists move together.
const CURRENT: &str = "temperature_2m,apparent_temperature,relative_humidity_2m,is_day,\
                       weather_code,wind_speed_10m,wind_direction_10m,precipitation";
const HOURLY: &str = "temperature_2m,precipitation_probability,weather_code,is_day";
const DAILY: &str = "weather_code,temperature_2m_max,temperature_2m_min,\
                     precipitation_probability_max,sunrise,sunset";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Forecast {
    /// Local times are in this zone (`timezone=auto`); every time string in
    /// the response is local wall-clock time without an offset.
    pub timezone: String,
    pub utc_offset_seconds: i32,
    pub current: Current,
    pub hourly: Hourly,
    pub daily: Daily,
    /// Units the request asked for, so a cached forecast is never shown
    /// under the other unit's suffix.
    #[serde(default)]
    pub units: Units,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Current {
    pub time: String,
    pub temperature_2m: f32,
    pub apparent_temperature: f32,
    pub relative_humidity_2m: f32,
    pub is_day: u8,
    pub weather_code: u8,
    pub wind_speed_10m: f32,
    pub wind_direction_10m: f32,
    pub precipitation: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hourly {
    pub time: Vec<String>,
    pub temperature_2m: Vec<Option<f32>>,
    pub precipitation_probability: Vec<Option<f32>>,
    pub weather_code: Vec<Option<u8>>,
    pub is_day: Vec<Option<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Daily {
    pub time: Vec<String>,
    pub weather_code: Vec<Option<u8>>,
    pub temperature_2m_max: Vec<Option<f32>>,
    pub temperature_2m_min: Vec<Option<f32>>,
    pub precipitation_probability_max: Vec<Option<f32>>,
    pub sunrise: Vec<String>,
    pub sunset: Vec<String>,
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("cce-weather/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

pub fn fetch_forecast(loc: &Location, units: Units) -> Result<Forecast, String> {
    let lat = format!("{:.4}", loc.latitude);
    let lon = format!("{:.4}", loc.longitude);
    let mut query: Vec<(&str, &str)> = vec![
        ("latitude", &lat),
        ("longitude", &lon),
        ("current", CURRENT),
        ("hourly", HOURLY),
        ("daily", DAILY),
        ("timezone", "auto"),
        ("forecast_days", "7"),
    ];
    if units == Units::Imperial {
        query.extend([
            ("temperature_unit", "fahrenheit"),
            ("wind_speed_unit", "mph"),
            ("precipitation_unit", "inch"),
        ]);
    }
    let resp = client()?
        .get(FORECAST_URL)
        .query(&query)
        .send()
        .map_err(|e| format!("network: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("forecast: HTTP {}", resp.status()));
    }
    let mut f: Forecast = resp.json().map_err(|e| format!("forecast: {e}"))?;
    f.units = units;
    Ok(f)
}

#[derive(Deserialize)]
struct GeoResponse {
    #[serde(default)]
    results: Vec<GeoResult>,
}

#[derive(Deserialize)]
struct GeoResult {
    name: String,
    latitude: f64,
    longitude: f64,
    #[serde(default)]
    admin1: Option<String>,
    #[serde(default)]
    country: Option<String>,
}

/// Up to `count` places matching a free-text name, best match first.
pub fn geocode(query: &str, count: usize) -> Result<Vec<Location>, String> {
    let count = count.to_string();
    let resp = client()?
        .get(GEOCODE_URL)
        .query(&[("name", query), ("count", &count), ("format", "json")])
        .send()
        .map_err(|e| format!("network: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("search: HTTP {}", resp.status()));
    }
    let geo: GeoResponse = resp.json().map_err(|e| format!("search: {e}"))?;
    Ok(geo
        .results
        .into_iter()
        .map(|r| Location {
            name: r.name,
            region: [r.admin1, r.country].into_iter().flatten().collect::<Vec<_>>().join(", "),
            latitude: r.latitude,
            longitude: r.longitude,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed real response: the shape serde must accept, including the
    /// nulls Open-Meteo sends past the end of a model's horizon.
    const SAMPLE: &str = r#"{
        "latitude": 40.71, "longitude": -74.0, "timezone": "America/New_York",
        "utc_offset_seconds": -14400,
        "current": {"time": "2026-10-05T14:00", "interval": 900, "temperature_2m": 18.4,
            "apparent_temperature": 17.1, "relative_humidity_2m": 61, "is_day": 1,
            "weather_code": 2, "wind_speed_10m": 11.2, "wind_direction_10m": 225,
            "precipitation": 0.0},
        "hourly": {"time": ["2026-10-05T00:00", "2026-10-05T01:00"],
            "temperature_2m": [15.0, null], "precipitation_probability": [10, null],
            "weather_code": [3, null], "is_day": [0, 0]},
        "daily": {"time": ["2026-10-05"], "weather_code": [61],
            "temperature_2m_max": [19.2], "temperature_2m_min": [12.0],
            "precipitation_probability_max": [70],
            "sunrise": ["2026-10-05T07:01"], "sunset": ["2026-10-05T18:37"]}
    }"#;

    #[test]
    fn parses_forecast_with_nulls() {
        let f: Forecast = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(f.current.weather_code, 2);
        assert_eq!(f.hourly.temperature_2m[1], None);
        assert_eq!(f.daily.precipitation_probability_max[0], Some(70.0));
        assert_eq!(f.units, Units::Metric);
    }

    /// Against the real service: `cargo test -p cce-weather -- --ignored`.
    #[test]
    #[ignore]
    fn live_geocode_and_forecast() {
        let places = geocode("Brooklyn", 3).unwrap();
        assert!(!places.is_empty());
        let f = fetch_forecast(&places[0], Units::Imperial).unwrap();
        assert!(f.hourly.time.len() >= 24 * 7);
        assert_eq!(f.daily.time.len(), 7);
        assert_eq!(f.units, Units::Imperial);
    }
}
