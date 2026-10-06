# cce-weather

Current conditions, the next 24 hours and the week ahead. Read the workspace
guide (`../cce-compositor/WORKSPACE.md`) first; this file covers only what is
particular to this crate.

## Shape

- `src/main.rs` — the `Application`: root plate, a control row (place
  search `TextBox`, unit toggle, refresh), then three pane plates — now, the
  hourly chart (a canvas well: glyphs, curve with labels, precipitation-chance
  bars, hour labels) and the daily rows (range bar on the week's scale,
  coloured cold → warm). A search with several matches swaps the panes for
  up to five list-row buttons; one match is taken without asking.
- `src/api.rs` — Open-Meteo forecast + geocoder over blocking reqwest, on
  worker threads; results come back through the loop's `Sender`. The
  requested field lists and the serde structs move together.
- `src/config.rs` — config, state, units.
- `src/glyph.rs` — WMO code → words and → glyph. The glyphs are the
  cce-icons set's multicolour `weather-*` family (sun, moon, clouds, rain,
  snow, bolt in their own colours), drawn with `PaintCtx::icon_untinted` —
  never `icon`, which would tint them one colour. Clear and partly cloudy
  have a night variant (`is_day`); the rest are one glyph for both. Every
  icon this app draws comes from that set; the refresh button's text
  fallback is the word "Refresh". Without `CCE_ICONS_DIR` reaching the set
  (a shadow's isolated HOME), every glyph draws blank.

## Data

Open-Meteo needs no key or account. `timezone=auto` makes every time string
local wall-clock time *at the place*, without an offset — compare them as
strings (`hour_index`), never against the machine's clock.

## Config vs state

- `~/.config/cce/cce-weather/config.kdl`:
  `units "imperial"|"metric"`, `location "Name" lat=… lon=… region="…"`,
  `refresh_minutes 15` (clamped 5..1440). No units anywhere → the
  measurement locale (US → imperial).
- `~/.local/state/cce/weather/state.json`: ONLY choices made in the app (a
  searched place, the toggle) plus the last forecast and the place/units it
  was for. A choice made in the app wins over the config; a setting never
  touched in the app keeps following the config. Delete the state file to
  go back to the config entirely.

## Refresh

A timer thread wakes every minute and compares the *wall clock* with the
last fetch, so a laptop waking from suspend refreshes within a minute — a
monotonic sleep for the whole interval does not count suspended time. A
failed fetch also resets the clock (one retry per interval, not per minute);
Refresh / Ctrl+R / F5 retry at once. The idle frame loop is untouched: the
thread only sends a message when a refresh is due.

## Text

Pass the family from `list_font_parsed().0`, never `list_font()`: the
latter may carry a size, which overrides every size asked for (the big
temperature rendered at body size until this).

## Verifying

`cargo test -p cce-weather` (offline); `cargo test -p cce-weather -- --ignored`
hits the real service. Visually, shadow only, with the fonts exported:

```sh
cce-shadow start --new            # add --scale 2 for the live display's scale
cce-shadow spawn env CCE_FONTS_DIR=/home/lsgalante/Dropbox/Fonts \
  CCE_ICONS_DIR=/home/lsgalante/projects/cce/cce-icons/svg \
  /home/lsgalante/projects/cce/target/release/cce-weather
cce-shadow shot-window 0
```

The shadow has its own HOME, so its state file is not the real one.
