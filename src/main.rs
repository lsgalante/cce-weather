//! cce-weather — current conditions, the next 24 hours and the week ahead,
//! from Open-Meteo (no account, no key).
//!
//! The window is a root plate with a control row on it (place search, unit
//! toggle, refresh) and three pane plates below: now, the hourly chart (a
//! canvas well), and the daily rows. A search with several matches swaps
//! the panes for a list of them. Network work runs on worker threads and
//! comes back through the loop's `Sender`; a timer thread asks for a refresh
//! by wall clock, so a forecast is not left stale across a suspend.

mod api;
mod config;
mod glyph;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use wayland_client::QueueHandle;

use cce_ui::widget::Owned;
use cce_ui::engine::{Application, EngineState, LogicalPosition, LogicalSize, WindowSettings};
use cce_ui::layout::{
    align_text_y, bevel_width, control_gap, list_font_parsed, plate_corner_radius,
    plate_padding,
};
use cce_ui::scene::arena::Arena;
use cce_ui::scene::layout::{compute_layout, LayoutBox, Length, Rect, Size as LSize, Style};
use cce_ui::scene::paint::{Cap, DisplayList, PaintCtx};
use cce_ui::scene::Material;
use cce_ui::widget::{
    Adapted, Button, ElementState, Event, Key, KeyEvent, MouseButton, MouseScrollDelta, NamedKey,
    TextBox, WidgetHost, WidgetId,
};

use api::Forecast;
use config::{Location, State, Units};

/// How many geocoder matches the picker offers.
const MAX_RESULTS: usize = 5;
/// Fixed pane heights; the daily pane takes what is left.
const NOW_H: f32 = 156.0;
const HOURLY_H: f32 = 172.0;
const HOURS_SHOWN: usize = 24;
/// The hourly chart labels every this many hours.
const LABEL_EVERY: usize = 3;

const BIG_SIZE: f32 = 46.0;
const HEAD_SIZE: f32 = 15.0;

const PRECIP: [u8; 3] = [110, 170, 255];
// Prim colours below are sRGB and go through `lin` where drawn; text takes
// sRGB bytes directly.
const PRECIP_BAR: [f32; 4] = [0.36, 0.62, 1.0, 0.45];
const CURVE: [f32; 4] = [1.0, 0.68, 0.30, 1.0];
const COLD: [f32; 3] = [0.40, 0.66, 1.0];
const WARM: [f32; 3] = [1.0, 0.60, 0.26];
const TRACK: [f32; 4] = [1.0, 1.0, 1.0, 0.08];

#[derive(Debug, Clone)]
enum Message {
    Forecast { generation: u64, result: Result<Forecast, String> },
    Geocoded { query: String, result: Result<Vec<Location>, String> },
    /// The timer thread: a refresh is due by wall clock.
    RefreshDue,
    Exit,
}

fn shaped_width(text: &str, size: f32, font: &str) -> f32 {
    let mut fs = cce_ui::geometry_font_system().lock().unwrap_or_else(|e| e.into_inner());
    cce_ui::backend::window_runner::shaped_cluster_offsets(&mut fs, text, size, Some(font))
        .last()
        .map_or(0.0, |&(_, x)| x)
}

fn to_u8(c: [f32; 4]) -> [u8; 3] {
    [(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8]
}

fn srgb_u8(linear: [f32; 4]) -> [u8; 3] {
    to_u8(cce_ui::colors::to_srgb(linear))
}

fn lin(c: [f32; 4]) -> [f32; 4] {
    cce_ui::color::to_linear(c)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn parse_local(t: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M").ok()
}

fn compass(deg: f32) -> &'static str {
    const POINTS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    POINTS[(((deg.rem_euclid(360.0) + 22.5) / 45.0) as usize) % 8]
}

/// Hour label in the unit system's habit: 24-hour for metric, "3pm" else.
fn hour_label(t: &NaiveDateTime, units: Units) -> String {
    match units {
        Units::Metric => format!("{:02}", t.hour()),
        Units::Imperial => {
            let h = t.hour() % 12;
            format!("{}{}", if h == 0 { 12 } else { h }, if t.hour() < 12 { "am" } else { "pm" })
        }
    }
}

fn clock(t: &NaiveDateTime, units: Units) -> String {
    match units {
        Units::Metric => t.format("%H:%M").to_string(),
        Units::Imperial => t.format("%-I:%M %P").to_string(),
    }
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, 1.0]
}

/// The index of the hour containing `now` in an hourly series: the first
/// slot whose hour is not before it (ISO strings compare as times).
fn hour_index(times: &[String], now: &str) -> usize {
    let key = &now[..now.len().min(13)];
    times.iter().position(|t| &t[..t.len().min(13)] >= key).unwrap_or(0)
}

/// Spawn the timer: every minute it checks the wall clock against the last
/// fetch, and says so once a refresh is due. Wall clock, not a sleep for the
/// whole interval, because a monotonic sleep does not count a suspend.
fn spawn_timer(sender: calloop::channel::Sender<Message>, last_fetch: Arc<AtomicU64>, every_min: u64) {
    std::thread::Builder::new()
        .name("weather-timer".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
            let last = last_fetch.load(Ordering::Relaxed) as i64;
            if now_unix() - last >= (every_min * 60) as i64 && sender.send(Message::RefreshDue).is_err() {
                return;
            }
        })
        .expect("spawning the refresh timer");
}

struct WeatherApp {
    // Widgets: plain fields so their addresses are stable (the UiContext
    // registry holds pointers to them).
    search: Owned<Adapted<TextBox>>,
    units_btn: Owned<Adapted<Button>>,
    refresh_btn: Owned<Adapted<Button>>,
    results_btns: [Owned<Adapted<Button>>; MAX_RESULTS],

    // App state — the source of truth; widgets are re-asserted from it.
    location: Option<Location>,
    units: Units,
    /// What the user picked in this app (a search, the toggle) — the only
    /// parts of `location`/`units` that are saved; see `config::State`.
    chosen_location: Option<Location>,
    chosen_units: Option<Units>,
    forecast: Option<Forecast>,
    fetched_at: Option<i64>,
    /// Bumped whenever the place or units change, so a reply for the old
    /// request is dropped on arrival.
    generation: u64,
    loading: bool,
    searching: bool,
    results: Vec<Location>,
    status: Option<String>,
    last_fetch: Arc<AtomicU64>,
    sender: calloop::channel::Sender<Message>,

    ui_context: cce_ui::context::UiContext,
    widgets_registered: bool,
    needs_rebuild: bool,
    size: (f32, f32),
    scale: f64,
    // Solved geometry, kept between rebuilds for painting.
    now_rect: Rect,
    hourly_rect: Rect,
    daily_rect: Rect,
    results_rect: Rect,
}

impl WeatherApp {
    fn root_ids(&self) -> Vec<WidgetId> {
        let mut ids = vec![self.search.id(), self.units_btn.id(), self.refresh_btn.id()];
        ids.extend(self.results_btns.iter().map(|b| b.id()));
        ids
    }

    fn picking(&self) -> bool {
        !self.results.is_empty()
    }

    fn save(&self) {
        config::save_state(&State {
            location: self.chosen_location.clone(),
            units: self.chosen_units,
            forecast: self.forecast.clone(),
            forecast_location: self.forecast.as_ref().and(self.location.clone()),
            fetched_at: self.fetched_at,
        });
    }

    fn refresh(&mut self) {
        let Some(loc) = self.location.clone() else { return };
        self.loading = true;
        self.needs_rebuild = true;
        let (generation, units, sender) = (self.generation, self.units, self.sender.clone());
        std::thread::spawn(move || {
            let result = api::fetch_forecast(&loc, units);
            let _ = sender.send(Message::Forecast { generation, result });
        });
    }

    fn set_location(&mut self, loc: Location) {
        self.chosen_location = Some(loc.clone());
        self.location = Some(loc);
        self.forecast = None;
        self.fetched_at = None;
        self.generation += 1;
        self.results.clear();
        self.status = None;
        self.save();
        self.refresh();
    }

    fn toggle_units(&mut self) {
        self.units = self.units.toggled();
        self.chosen_units = Some(self.units);
        self.generation += 1;
        self.save();
        self.refresh();
    }

    fn search_value(&self) -> String {
        let raw = if self.search.editing { &self.search.edit_buffer } else { &self.search.text };
        raw.trim().to_string()
    }

    fn clear_search(&mut self) {
        self.search.text.clear();
        self.search.edit_buffer.clear();
        self.search.cursor_idx = 0;
    }

    fn submit_search(&mut self) {
        let query = self.search_value();
        if query.is_empty() {
            return;
        }
        self.searching = true;
        self.status = None;
        self.needs_rebuild = true;
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let result = api::geocode(&query, MAX_RESULTS);
            let _ = sender.send(Message::Geocoded { query, result });
        });
    }

    fn drain_widget_changes(&mut self) {
        if self.units_btn.take_click() {
            self.toggle_units();
        }
        if self.refresh_btn.take_click() {
            self.refresh();
        }
        for i in 0..MAX_RESULTS {
            if self.results_btns[i].take_click() {
                if let Some(loc) = self.results.get(i).cloned() {
                    self.clear_search();
                    self.search.unfocus();
                    self.set_location(loc);
                }
            }
        }
        if self.search.take_change() {
            self.needs_rebuild = true;
        }
    }

    fn route(&mut self, ev: &Event, short_circuit: bool) -> bool {
        let mut changed = false;
        for id in self.root_ids() {
            if self.ui_context.propagate_event(ev, id) {
                changed = true;
                if short_circuit {
                    break;
                }
            }
        }
        self.drain_widget_changes();
        changed
    }

    fn layout(&mut self) {
        let (w, h) = self.size;
        let mut arena: Arena<LayoutBox> = Arena::new();
        let root = arena.insert(LayoutBox::container(Style::root_column()));
        let box_h = cce_ui::layout::textbox_height();
        let btn_h = cce_ui::layout::button_height();
        let controls = arena.insert(LayoutBox::container(
            Style::controls_row().height(Length::Fixed(box_h.max(btn_h))),
        ));
        let search = arena.insert(LayoutBox::leaf(Style::row().grow(1.0).shrink(1.0), LSize::new(0.0, box_h)));
        let units = arena.insert(LayoutBox::leaf(Style::row(), LSize::new(52.0, btn_h)));
        let refresh = arena.insert(LayoutBox::leaf(Style::row(), LSize::new(btn_h, btn_h)));
        let now = arena.insert(LayoutBox::leaf(Style::row(), LSize::new(0.0, NOW_H)));
        let hourly = arena.insert(LayoutBox::leaf(Style::row(), LSize::new(0.0, HOURLY_H)));
        let daily = arena.insert(LayoutBox::container(Style::column().grow(1.0)));
        arena.append_child(root, controls);
        arena.append_child(controls, search);
        arena.append_child(controls, units);
        arena.append_child(controls, refresh);
        arena.append_child(root, now);
        arena.append_child(root, hourly);
        arena.append_child(root, daily);
        compute_layout(&mut arena, root, LSize::new(w, h));

        let r = |id| arena.value(id).unwrap().rect;
        let s = r(search);
        self.search.set_rect(s.x, s.y, s.width, s.height);
        let u = r(units);
        self.units_btn.set_rect(u.x, u.y, u.width, u.height);
        let f = r(refresh);
        self.refresh_btn.set_rect(f.x, f.y, f.width, f.height);
        self.now_rect = r(now);
        self.hourly_rect = r(hourly);
        self.daily_rect = r(daily);

        // The picker takes the whole area under the control row.
        let top = self.now_rect.y;
        let bottom = self.daily_rect.y + self.daily_rect.height;
        self.results_rect = Rect { x: self.now_rect.x, y: top, width: self.now_rect.width, height: bottom - top };
        let pad = plate_padding();
        let row_h = btn_h;
        let picking = self.picking();
        for (i, b) in self.results_btns.iter_mut().enumerate() {
            match self.results.get(i) {
                Some(loc) if picking => {
                    let label = if loc.region.is_empty() {
                        loc.name.clone()
                    } else {
                        format!("{} — {}", loc.name, loc.region)
                    };
                    b.set_label(&label);
                    let y = top + pad + HEAD_SIZE * 1.6 + i as f32 * (row_h + control_gap());
                    b.set_rect(self.results_rect.x + pad, y, self.results_rect.width - 2.0 * pad, row_h);
                }
                // Off the window, so a hidden row takes no clicks.
                _ => b.set_rect(-1000.0, -1000.0, 0.0, 0.0),
            }
        }
    }

    fn pane(&self, pc: &mut PaintCtx, rect: Rect) {
        let r = plate_corner_radius();
        pc.plate(rect, (r, r, r, r), &Material::pane(), bevel_width().min(rect.height * 0.2));
    }

    fn paint_now(&self, pc: &mut PaintCtx, font: &str, size: f32, fg: [u8; 3], dim: [u8; 3]) {
        let rect = self.now_rect;
        self.pane(pc, rect);
        let pad = plate_padding();
        let inner = Rect {
            x: rect.x + pad,
            y: rect.y + pad,
            width: rect.width - 2.0 * pad,
            height: rect.height - 2.0 * pad,
        };
        let text = |pc: &mut PaintCtx, s: String, x: f32, y: f32, sz: f32, c: [u8; 3]| {
            pc.text_with(s, x, y, sz, c, Some(font.to_string()), Some([inner.x, inner.y, inner.x + inner.width, inner.y + inner.height]));
        };

        let Some(loc) = &self.location else {
            text(pc, "No place yet".into(), inner.x, inner.y, HEAD_SIZE, fg);
            text(pc, "Type a city in the box above and press Enter.".into(), inner.x, inner.y + HEAD_SIZE * 1.6, size, dim);
            return;
        };

        // The place, right-aligned along the top.
        let place = loc.name.clone();
        let pw = shaped_width(&place, HEAD_SIZE, font);
        text(pc, place, inner.x + inner.width - pw, inner.y, HEAD_SIZE, fg);
        if !loc.region.is_empty() {
            let rw = shaped_width(&loc.region, size, font);
            text(pc, loc.region.clone(), inner.x + inner.width - rw, inner.y + HEAD_SIZE * 1.4, size, dim);
        }

        let Some(f) = &self.forecast else {
            let msg = if self.loading { "Loading…" } else { "No forecast" };
            text(pc, msg.into(), inner.x, inner.y, HEAD_SIZE, dim);
            return;
        };
        let c = &f.current;
        let gs = inner.height.min(96.0);
        glyph::draw(pc, Rect { x: inner.x, y: inner.y + (inner.height - gs) / 2.0, width: gs, height: gs }, c.weather_code, c.is_day != 0);

        let x = inner.x + gs + pad;
        let temp = format!("{:.0}°", c.temperature_2m);
        text(pc, temp, x, inner.y, BIG_SIZE, fg);
        let (_, words) = glyph::describe(c.weather_code);
        let line = size * 1.45;
        let mut y = inner.y + BIG_SIZE * 1.25;
        text(pc, words.to_string(), x, y, HEAD_SIZE, fg);
        y += HEAD_SIZE * 1.45;
        text(pc, format!("Feels like {:.0}°  ·  Humidity {:.0}%", c.apparent_temperature, c.relative_humidity_2m), x, y, size, dim);
        y += line;
        let mut detail = format!("Wind {:.0} {} {}", c.wind_speed_10m, f.units.wind_suffix(), compass(c.wind_direction_10m));
        if let (Some(hi), Some(lo)) = (
            f.daily.temperature_2m_max.first().copied().flatten(),
            f.daily.temperature_2m_min.first().copied().flatten(),
        ) {
            detail.push_str(&format!("  ·  H {hi:.0}°  L {lo:.0}°"));
        }
        text(pc, detail, x, y, size, dim);

        // Freshness, bottom-right: when the numbers are from, or what went wrong.
        let foot = if self.loading {
            "Updating…".to_string()
        } else if let Some(e) = &self.status {
            e.clone()
        } else if let Some(t) = parse_local(&c.time) {
            let age = self.fetched_at.map_or(0, |at| now_unix() - at);
            if age > 3 * 3600 {
                format!("{} · {}h old", clock(&t, f.units), age / 3600)
            } else {
                format!("As of {}", clock(&t, f.units))
            }
        } else {
            String::new()
        };
        let small = (size - 1.0).max(9.0);
        let fw = shaped_width(&foot, small, font);
        let fy = inner.y + HEAD_SIZE * 1.4 + if loc.region.is_empty() { 0.0 } else { size * 1.4 };
        text(pc, foot, inner.x + inner.width - fw, fy, small, dim);
    }

    fn paint_hourly(&self, pc: &mut PaintCtx, font: &str, size: f32, fg: [u8; 3], dim: [u8; 3]) {
        let rect = self.hourly_rect;
        self.pane(pc, rect);
        let pad = plate_padding();
        pc.text_with("Next 24 hours", rect.x + pad, rect.y + pad, size, fg, Some(font.to_string()), None);
        let well = Rect {
            x: rect.x + pad,
            y: rect.y + pad + size * 1.6,
            width: rect.width - 2.0 * pad,
            height: rect.height - 2.0 * pad - size * 1.6,
        };
        let r = cce_ui::layout::textbox_corner_radius();
        pc.canvas_well(well, r, &Material::pane(), cce_ui::layout::control_relief(), false);

        let Some(f) = &self.forecast else { return };
        let start = hour_index(&f.hourly.time, &f.current.time);
        let end = (start + HOURS_SHOWN + 1).min(f.hourly.time.len());
        if end <= start + 1 {
            return;
        }
        let temps: Vec<Option<f32>> = f.hourly.temperature_2m[start..end].to_vec();
        let (lo, hi) = temps.iter().flatten().fold((f32::MAX, f32::MIN), |(a, b), &t| (a.min(t), b.max(t)));
        if lo > hi {
            return;
        }
        let span = (hi - lo).max(4.0);
        let mid = (hi + lo) / 2.0;
        let small = (size - 1.5).max(9.0);

        // Bands inside the well, top to bottom: glyphs, the curve (with its
        // labels above each point), the precipitation bars, the hour labels.
        let inset = r.max(6.0);
        let gl = 18.0f32;
        let x0 = well.x + inset;
        let x1 = well.x + well.width - inset;
        let glyph_y = well.y + inset * 0.5;
        let hours_y = well.y + well.height - inset - small * 1.2;
        let curve_top = glyph_y + gl + small * 1.4;
        let curve_bot = hours_y - small * 0.6;
        let n = end - start;
        let step = (x1 - x0) / (n - 1) as f32;
        let px = |i: usize| x0 + step * i as f32;
        let py = |t: f32| {
            let k = (t - (mid - span / 2.0)) / span;
            curve_bot - k * (curve_bot - curve_top)
        };

        // Precipitation chance: bars rising from the curve band's floor.
        let bar_max = (curve_bot - curve_top) * 0.45;
        let bar_w = (step * 0.6).max(2.0);
        for i in 0..n {
            if let Some(p) = f.hourly.precipitation_probability.get(start + i).copied().flatten() {
                if p >= 5.0 {
                    let bh = bar_max * p / 100.0;
                    pc.rounded_rect(
                        Rect { x: px(i) - bar_w / 2.0, y: curve_bot - bh, width: bar_w, height: bh },
                        (bar_w / 2.0).min(2.0),
                        (true, true, false, false),
                        lin(PRECIP_BAR),
                    );
                }
            }
        }

        // The curve, skipping across gaps rather than drawing to zero.
        let mut prev: Option<(f32, f32)> = None;
        for (i, t) in temps.iter().enumerate() {
            match t {
                Some(t) => {
                    let p = (px(i), py(*t));
                    if let Some(q) = prev {
                        pc.vector(q.0, q.1, p.0, p.1, 2.0, lin(CURVE), Cap::Round);
                    }
                    prev = Some(p);
                }
                None => prev = None,
            }
        }

        // Labels every few hours: glyph on top, temperature over its point,
        // the hour under the well's floor band.
        for i in (0..n).step_by(LABEL_EVERY) {
            let x = px(i);
            if let Some(t) = temps[i] {
                pc.circle(x, py(t), 3.0, lin(CURVE));
                let s = format!("{t:.0}°");
                let w = shaped_width(&s, small, font);
                let lx = (x - w / 2.0).clamp(well.x + inset * 0.5, well.x + well.width - inset * 0.5 - w);
                pc.text_with(s, lx, py(t) - small * 1.5, small, fg, Some(font.to_string()), None);
            }
            if let Some(code) = f.hourly.weather_code.get(start + i).copied().flatten() {
                let day = f.hourly.is_day.get(start + i).copied().flatten().unwrap_or(1) != 0;
                let gx = (x - gl / 2.0).clamp(well.x + inset * 0.5, well.x + well.width - inset * 0.5 - gl);
                glyph::draw(pc, Rect { x: gx, y: glyph_y, width: gl, height: gl }, code, day);
            }
            let label = if i == 0 {
                "Now".to_string()
            } else {
                parse_local(&f.hourly.time[start + i]).map(|t| hour_label(&t, f.units)).unwrap_or_default()
            };
            let w = shaped_width(&label, small, font);
            let lx = (x - w / 2.0).clamp(well.x + inset * 0.5, well.x + well.width - inset * 0.5 - w);
            pc.text_with(label, lx, hours_y, small, dim, Some(font.to_string()), None);
        }
    }

    fn paint_daily(&self, pc: &mut PaintCtx, font: &str, size: f32, fg: [u8; 3], dim: [u8; 3]) {
        let rect = self.daily_rect;
        if rect.height < 40.0 {
            return;
        }
        self.pane(pc, rect);
        let pad = plate_padding();
        pc.text_with("7 days", rect.x + pad, rect.y + pad, size, fg, Some(font.to_string()), None);
        let Some(f) = &self.forecast else { return };
        let d = &f.daily;
        let days = d.time.len().min(7);
        if days == 0 {
            return;
        }
        let week_lo = d.temperature_2m_min.iter().take(days).flatten().fold(f32::MAX, |a, &b| a.min(b));
        let week_hi = d.temperature_2m_max.iter().take(days).flatten().fold(f32::MIN, |a, &b| a.max(b));
        let week_span = (week_hi - week_lo).max(1.0);

        let top = rect.y + pad + size * 1.8;
        let bottom = rect.y + rect.height - pad;
        let row_h = ((bottom - top) / days as f32).min(44.0);
        let x0 = rect.x + pad;
        let x1 = rect.x + rect.width - pad;
        let today = d.time.first().and_then(|t| NaiveDate::parse_from_str(t, "%Y-%m-%d").ok());

        // Columns: day | glyph | chance of rain | low ─ bar ─ high.
        let day_w = shaped_width("Today", size, font).max(shaped_width("Wed", size, font)) + pad;
        let gs = (row_h * 0.62).min(26.0);
        let pct_w = shaped_width("100%", size, font) + pad;
        let num_w = shaped_width("-00°", size, font);
        let bar_x0 = x0 + day_w + gs + pad + pct_w + num_w + pad * 0.5;
        let bar_x1 = x1 - num_w - pad * 0.5;

        for i in 0..days {
            let y = top + row_h * i as f32;
            let ty = align_text_y(y, row_h, size, 0.0);
            let date = NaiveDate::parse_from_str(&d.time[i], "%Y-%m-%d").ok();
            let name = match (date, today) {
                (Some(dt), Some(t0)) if dt == t0 => "Today".to_string(),
                (Some(dt), _) => dt.weekday().to_string(),
                _ => d.time[i].clone(),
            };
            pc.text_with(name, x0, ty, size, fg, Some(font.to_string()), None);
            if let Some(code) = d.weather_code[i] {
                glyph::draw(pc, Rect { x: x0 + day_w, y: y + (row_h - gs) / 2.0, width: gs, height: gs }, code, true);
            }
            if let Some(p) = d.precipitation_probability_max[i] {
                if p >= 10.0 {
                    pc.text_with(format!("{p:.0}%"), x0 + day_w + gs + pad, ty, size, PRECIP, Some(font.to_string()), None);
                }
            }
            let (Some(lo), Some(hi)) = (d.temperature_2m_min[i], d.temperature_2m_max[i]) else { continue };
            let lo_s = format!("{lo:.0}°");
            let lw = shaped_width(&lo_s, size, font);
            pc.text_with(lo_s, bar_x0 - pad * 0.5 - lw, ty, size, dim, Some(font.to_string()), None);
            pc.text_with(format!("{hi:.0}°"), bar_x1 + pad * 0.5, ty, size, fg, Some(font.to_string()), None);

            // The day's range on the week's scale, coloured cold → warm.
            if bar_x1 - bar_x0 > 12.0 {
                let bh = 5.0f32;
                let by = y + (row_h - bh) / 2.0;
                let track = Rect { x: bar_x0, y: by, width: bar_x1 - bar_x0, height: bh };
                pc.rounded_rect(track, bh / 2.0, (true, true, true, true), TRACK);
                let a = (lo - week_lo) / week_span;
                let b = (hi - week_lo) / week_span;
                let seg = Rect {
                    x: bar_x0 + a * track.width,
                    y: by,
                    width: ((b - a) * track.width).max(bh),
                    height: bh,
                };
                pc.rounded_rect(seg, bh / 2.0, (true, true, true, true), lin(lerp3(COLD, WARM, (a + b) / 2.0)));
            }
        }
    }

    fn paint_results(&self, pc: &mut PaintCtx, font: &str, size: f32, fg: [u8; 3]) {
        let rect = self.results_rect;
        self.pane(pc, rect);
        let pad = plate_padding();
        pc.text_with("Choose a place  (Esc to cancel)", rect.x + pad, rect.y + pad, size, fg, Some(font.to_string()), None);
        for b in &self.results_btns {
            cce_ui::scene::painter::paint_root_into(&self.ui_context, b, pc);
        }
    }
}

impl Application for WeatherApp {
    type Message = Message;

    fn new(_qh: &QueueHandle<EngineState<Self>>, sender: calloop::channel::Sender<Self::Message>) -> Self {
        let config = config::load_config();
        let state = config::load_state();
        let units = config::resolve_units(&state, &config);
        let location = state.location.clone().or_else(|| config.location.clone());
        // A cached forecast is shown only for the place and units it was for.
        let cache_ok = location.is_some()
            && state.forecast_location == location
            && state.forecast.as_ref().is_some_and(|f| f.units == units);
        let (forecast, fetched_at) = if cache_ok { (state.forecast, state.fetched_at) } else { (None, None) };
        let last_fetch = Arc::new(AtomicU64::new(fetched_at.unwrap_or(0) as u64));
        spawn_timer(sender.clone(), last_fetch.clone(), config.refresh_minutes);

        let mut app = Self {
            search: Owned::new(TextBox::new(String::new()).with_placeholder("Search for a city…")),
            units_btn: Owned::new(Button::new(0.0, 0.0, 0.0, 0.0).with_label(units.toggled().temp_suffix())),
            refresh_btn: Owned::new(Button::new(0.0, 0.0, 0.0, 0.0).with_icon_name("refresh", "Refresh")),
            results_btns: std::array::from_fn(|_| Owned::new(Button::new_list_row(0.0, 0.0, 0.0, 0.0).with_label(""))),
            chosen_location: state.location.clone(),
            chosen_units: state.units,
            location,
            units,
            forecast,
            fetched_at,
            generation: 0,
            loading: false,
            searching: false,
            results: Vec::new(),
            status: None,
            last_fetch,
            sender,
            ui_context: cce_ui::context::UiContext::new(),
            widgets_registered: false,
            needs_rebuild: true,
            size: (520.0, 720.0),
            scale: 1.0,
            now_rect: Rect::ZERO,
            hourly_rect: Rect::ZERO,
            daily_rect: Rect::ZERO,
            results_rect: Rect::ZERO,
        };
        // Always fetch at start: the cache is only for the first frame.
        app.refresh();
        app
    }

    fn settings(&self) -> WindowSettings {
        WindowSettings {
            title: "Weather".to_string(),
            app_id: "cce-weather".to_string(),
            width: 520,
            height: 720,
            fullscreen: false,
            min_size: Some((400, 560)),
        }
    }

    fn update(&mut self, msg: Self::Message, needs_rebuild: &mut bool, exit: &mut bool) {
        match msg {
            Message::Forecast { generation, result } => {
                if generation != self.generation {
                    return;
                }
                self.loading = false;
                match result {
                    Ok(f) => {
                        self.forecast = Some(f);
                        let now = now_unix();
                        self.fetched_at = Some(now);
                        self.last_fetch.store(now as u64, Ordering::Relaxed);
                        self.status = None;
                        self.save();
                    }
                    Err(e) => {
                        log::warn!("forecast: {e}");
                        // Back off a full interval rather than retrying every
                        // minute while offline; the button retries at once.
                        self.last_fetch.store(now_unix() as u64, Ordering::Relaxed);
                        self.status = Some("Couldn't update — offline?".into());
                    }
                }
            }
            Message::Geocoded { query, result } => {
                self.searching = false;
                match result {
                    Ok(list) if list.is_empty() => self.status = Some(format!("No place called “{query}”")),
                    // One match needs no question.
                    Ok(mut list) if list.len() == 1 => {
                        self.clear_search();
                        self.search.unfocus();
                        self.set_location(list.remove(0));
                    }
                    Ok(list) => self.results = list,
                    Err(e) => {
                        log::warn!("geocode: {e}");
                        self.status = Some("Search failed — offline?".into());
                    }
                }
            }
            Message::RefreshDue => {
                if !self.loading {
                    self.refresh();
                }
            }
            Message::Exit => *exit = true,
        }
        self.needs_rebuild = true;
        *needs_rebuild = true;
    }

    fn tick(&mut self, dt: f32, needs_rebuild: &mut bool) {
        if self.ui_context.tick(dt) {
            *needs_rebuild = true;
        }
    }

    fn display_list(&mut self, size: LogicalSize, scale: f64) -> Option<DisplayList> {
        if !self.widgets_registered {
            self.widgets_registered = true;
            self.ui_context.register_host(&mut self.search);
            self.ui_context.register_host(&mut self.units_btn);
            self.ui_context.register_host(&mut self.refresh_btn);
            for b in self.results_btns.iter_mut() {
                self.ui_context.register_host(b);
            }
        }
        let size_changed = self.size != (size.width, size.height) || self.scale != scale;
        if self.needs_rebuild || size_changed {
            self.size = (size.width, size.height);
            self.scale = scale;
            cce_ui::scale::set_scale_factor(scale as f32);
            // The button names the unit it switches TO.
            self.units_btn.set_label(self.units.toggled().temp_suffix());
            self.search.set_placeholder(if self.searching { "Searching…" } else { "Search for a city…" });
            self.layout();
            self.needs_rebuild = false;
            self.ui_context.rebuild_spatial_grid();
        }

        // The family alone: `list_font()` may carry a size, which would
        // override every size asked for here.
        let (font, size_pt) = list_font_parsed();
        let fg = to_u8(cce_ui::colors::list_font_color());
        let dim = srgb_u8(cce_ui::colors::TEXT_DIM);

        let mut pc = PaintCtx::new();
        pc.root_plate(size.width, size.height);
        cce_ui::scene::painter::paint_root_into(&self.ui_context, &self.search, &mut pc);
        cce_ui::scene::painter::paint_root_into(&self.ui_context, &self.units_btn, &mut pc);
        cce_ui::scene::painter::paint_root_into(&self.ui_context, &self.refresh_btn, &mut pc);

        if self.picking() {
            self.paint_results(&mut pc, &font, size_pt, fg);
        } else {
            self.paint_now(&mut pc, &font, size_pt, fg, dim);
            self.paint_hourly(&mut pc, &font, size_pt, fg, dim);
            self.paint_daily(&mut pc, &font, size_pt, fg, dim);
        }
        // A search error has no forecast pane to sit in when there is no place yet.
        if self.location.is_none() {
            if let Some(s) = &self.status {
                let y = self.daily_rect.y + self.daily_rect.height - size_pt * 1.4;
                pc.text_with(s.clone(), self.daily_rect.x, y, size_pt, PRECIP, Some(font.clone()), None);
            }
        }
        Some(pc.finish())
    }

    fn display_list_text(&self) -> bool {
        true
    }

    fn ui_context(&self) -> Option<&cce_ui::context::UiContext> {
        Some(&self.ui_context)
    }

    fn ui_context_mut(&mut self) -> Option<&mut cce_ui::context::UiContext> {
        Some(&mut self.ui_context)
    }

    fn is_movable_root_plate_at(&self, px: f32, py: f32) -> bool {
        self.ui_context.drag_allowed_at(px, py)
    }

    fn clear_color(&self) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn handle_pointer_move(&mut self, pos: LogicalPosition, needs_rebuild: &mut bool) {
        // The shared context menu (the search box's) gets the pointer to itself
        // while open: its row highlight.
        if cce_ui::widget::context_menu::is_visible() {
            if cce_ui::widget::context_menu::cursor_moved(pos.x, pos.y) {
                self.needs_rebuild = true;
                *needs_rebuild = true;
            }
            return;
        }
        let ev = Event::PointerMove { x: pos.x, y: pos.y, local_x: pos.x, local_y: pos.y };
        if self.route(&ev, false) || self.needs_rebuild {
            self.needs_rebuild = true;
            *needs_rebuild = true;
        }
    }

    fn handle_mouse_input(
        &mut self,
        button: MouseButton,
        state: ElementState,
        pos: LogicalPosition,
        needs_rebuild: &mut bool,
    ) -> Option<Self::Message> {
        // The shared context menu a right-click on the search box opens takes
        // every click while open: a row runs, a press anywhere else dismisses it.
        // The toolkit leaves this routing to the app; without it the menu could
        // not be closed by clicking outside it, and its rows did nothing. A row's
        // edit (Paste, Cut) is drained like any other change to the box.
        if cce_ui::widget::context_menu::is_visible() {
            if cce_ui::widget::context_menu::mouse_input(button, state, pos.x, pos.y, Some(&mut self.ui_context)) {
                self.drain_widget_changes();
                self.needs_rebuild = true;
                *needs_rebuild = true;
            }
            return None;
        }
        let ev = Event::MouseButton { button, state, x: pos.x, y: pos.y, local_x: pos.x, local_y: pos.y };
        if self.route(&ev, false) || self.needs_rebuild {
            self.needs_rebuild = true;
            *needs_rebuild = true;
        }
        None
    }

    fn handle_mouse_wheel(&mut self, delta: &MouseScrollDelta, pos: LogicalPosition, needs_rebuild: &mut bool) {
        let ev = Event::MouseWheel { delta: delta.clone(), x: pos.x, y: pos.y, local_x: pos.x, local_y: pos.y };
        if self.route(&ev, false) || self.needs_rebuild {
            self.needs_rebuild = true;
            *needs_rebuild = true;
        }
    }

    fn handle_key_input(&mut self, event: &KeyEvent, needs_rebuild: &mut bool) -> Option<Self::Message> {
        if event.state == ElementState::Pressed && !event.repeat {
            if event.ctrl {
                if let Key::Character(ref c) = event.logical_key {
                    match c.as_str() {
                        "q" => return Some(Message::Exit),
                        "r" => {
                            self.refresh();
                            *needs_rebuild = true;
                            return None;
                        }
                        _ => {}
                    }
                }
            }
            match event.logical_key {
                Key::Named(NamedKey::F5) => {
                    self.refresh();
                    *needs_rebuild = true;
                    return None;
                }
                Key::Named(NamedKey::Escape) if self.picking() || self.search.editing => {
                    self.results.clear();
                    self.search.unfocus();
                    self.needs_rebuild = true;
                    *needs_rebuild = true;
                    return None;
                }
                // The box's own edit mode, not `focused(&ctx)`: a click
                // focuses through the thread-local registry, which the
                // UiContext's focused_widget never learns of.
                Key::Named(NamedKey::Enter) if self.search.editing => {
                    self.submit_search();
                    *needs_rebuild = true;
                    return None;
                }
                _ => {}
            }
        }
        // KeyInput short-circuits: the router hands keys to the focused
        // widget on every propagate call.
        let ev = Event::KeyInput(event.clone());
        if self.route(&ev, true) || self.needs_rebuild {
            self.needs_rebuild = true;
            *needs_rebuild = true;
        }
        None
    }
}

fn main() {
    env_logger::init();
    cce_ui::engine::run::<WeatherApp>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hour_index_finds_the_current_hour() {
        let times: Vec<String> = (0..24).map(|h| format!("2026-10-05T{h:02}:00")).collect();
        assert_eq!(hour_index(&times, "2026-10-05T14:15"), 14);
        assert_eq!(hour_index(&times, "2026-10-05T00:00"), 0);
        // Past the series: fall back to the start rather than panic.
        assert_eq!(hour_index(&times, "2026-10-06T03:00"), 0);
    }

    #[test]
    fn compass_points() {
        assert_eq!(compass(0.0), "N");
        assert_eq!(compass(350.0), "N");
        assert_eq!(compass(225.0), "SW");
        assert_eq!(compass(-90.0), "W");
    }

    #[test]
    fn hour_labels_follow_units() {
        let t = parse_local("2026-10-05T15:00").unwrap();
        assert_eq!(hour_label(&t, Units::Metric), "15");
        assert_eq!(hour_label(&t, Units::Imperial), "3pm");
        let m = parse_local("2026-10-05T00:00").unwrap();
        assert_eq!(hour_label(&m, Units::Imperial), "12am");
    }
}
