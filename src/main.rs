//! CLI entry. No arguments runs the menu bar app; `probe` runs the read-only
//! verification harness; `render-settings` draws a Settings page offscreen;
//! the update commands live in `update_cli`.

mod update_cli;

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use ice_rust::agent_bar::{AgentSlot, is_folded};
use ice_rust::model::{DIVIDER_AUTOSAVE, ICON_AUTOSAVE, Rect, WindowRecord};
use ice_rust::worker::BoundedError;

const USAGE: &str = "\
usage:
  ice-rust                  run the menu bar app
  ice-rust probe [--divider <autosave-title>] [--out <dir>] [--json]
  ice-rust render-settings --page <general|hidden-items|layout|permissions|about> --out <png>
                           [--width W --height H] [--divider-prefix <prefix>]
                           [--default-settings]
  ice-rust check-update [--json]
  ice-rust apply-update --app <IceRust.app> --zip <zip> --sig <sig> [--version V]
  ice-rust update-keygen --out <private-key-file>
  ice-rust update-sign --key <private-key-file> <zip>
  ice-rust update-verify <zip>
  ice-rust --help

probe is read-only: it lists windows, walks the hidden, always-hidden and
visible chains from the dividers, and captures the hidden items. It posts no
events, creates no status items and writes no defaults. With --out it writes
one PNG per captured hidden item into <dir>.

render-settings builds the Settings window offscreen and writes one page as
a PNG. It shows no window, creates no status items and posts no events. The
General page shows the stored settings, or the defaults with
--default-settings. The layout page reads the live menu bar through the dividers titled
<prefix>Hidden and <prefix>AlwaysHidden; without --divider-prefix it tries
IceRust.ControlItem. and then Ice.ControlItem.

check-update reads the latest GitHub release once and reports whether it is
newer. apply-update verifies the zip's Ed25519 signature with the key built
into this binary, then installs it into the given bundle without relaunching.
update-keygen writes a new private key (mode 600, never overwrites) and
prints the public key to build in; update-sign writes <zip>.sig;
update-verify checks <zip>.sig against the key built into this binary.";

/// Page names `render-settings` accepts.
const PAGES: [&str; 5] = ["general", "hidden-items", "layout", "permissions", "about"];
/// Default Settings window content size.
const DEFAULT_RENDER_SIZE: (f64, f64) = (820.0, 600.0);

#[derive(Debug, PartialEq)]
enum Command {
    Gui,
    Probe(ProbeOptions),
    Render(RenderArgs),
    Update(update_cli::UpdateCommand),
    Help,
}

#[derive(Debug, PartialEq)]
struct RenderArgs {
    page: String,
    out: PathBuf,
    width: f64,
    height: f64,
    prefix: Option<String>,
    default_settings: bool,
}

fn parse_size(flag: &str, value: Option<&String>) -> Result<f64, String> {
    let value = value.ok_or(format!("{flag} needs a number"))?;
    match value.parse::<f64>() {
        Ok(v) if v.is_finite() && (200.0..=4000.0).contains(&v) => Ok(v),
        _ => Err(format!("{flag} must be a number from 200 to 4000")),
    }
}

fn parse_render(rest: &[String]) -> Result<Command, String> {
    let mut page = None;
    let mut out = None;
    let (mut width, mut height) = DEFAULT_RENDER_SIZE;
    let mut prefix = None;
    let mut default_settings = false;
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--page" => {
                let value = it.next().ok_or("--page needs a value")?;
                if !PAGES.contains(&value.as_str()) {
                    return Err(format!(
                        "unknown page {value}; use one of {}",
                        PAGES.join(", ")
                    ));
                }
                page = Some(value.clone());
            }
            "--out" => out = Some(PathBuf::from(it.next().ok_or("--out needs a path")?)),
            "--width" => width = parse_size("--width", it.next())?,
            "--height" => height = parse_size("--height", it.next())?,
            "--divider-prefix" => {
                let value = it.next().ok_or("--divider-prefix needs a value")?;
                if value.is_empty() {
                    return Err("--divider-prefix needs a non-empty value".into());
                }
                prefix = Some(value.clone());
            }
            "--default-settings" => default_settings = true,
            "-h" | "--help" => return Ok(Command::Help),
            other => return Err(format!("unknown render-settings argument: {other}")),
        }
    }
    Ok(Command::Render(RenderArgs {
        page: page.ok_or("render-settings needs --page")?,
        out: out.ok_or("render-settings needs --out")?,
        width,
        height,
        prefix,
        default_settings,
    }))
}

#[derive(Debug, PartialEq)]
struct ProbeOptions {
    divider: String,
    out: Option<PathBuf>,
    json: bool,
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    let Some((first, rest)) = args.split_first() else {
        return Ok(Command::Gui);
    };
    match first.as_str() {
        "-h" | "--help" | "help" => Ok(Command::Help),
        "probe" => {
            let mut options = ProbeOptions {
                divider: DIVIDER_AUTOSAVE.to_string(),
                out: None,
                json: false,
            };
            let mut it = rest.iter();
            while let Some(arg) = it.next() {
                match arg.as_str() {
                    "--divider" => {
                        let value = it.next().ok_or("--divider needs a value")?;
                        if value.is_empty() {
                            return Err("--divider needs a non-empty value".into());
                        }
                        options.divider = value.clone();
                    }
                    "--out" => {
                        let value = it.next().ok_or("--out needs a directory")?;
                        options.out = Some(PathBuf::from(value));
                    }
                    "--json" => options.json = true,
                    "-h" | "--help" => return Ok(Command::Help),
                    other => return Err(format!("unknown probe argument: {other}")),
                }
            }
            Ok(Command::Probe(options))
        }
        "render-settings" => parse_render(rest),
        other => match update_cli::parse(other, rest) {
            Some(parsed) => parsed.map(Command::Update),
            None => Err(format!("unknown command: {other}")),
        },
    }
}

/// The icon title that pairs with a divider title, used to pick the
/// divider copy on the active menu bar when several share the title.
/// `X.Hidden` pairs with `X.Visible`, which covers both our names and Ice's.
fn icon_title_for(divider: &str) -> String {
    match divider.strip_suffix(".Hidden") {
        Some(prefix) => format!("{prefix}.Visible"),
        None => ICON_AUTOSAVE.to_string(),
    }
}

/// The always-hidden divider title that pairs with a divider title:
/// `X.Hidden` pairs with `X.AlwaysHidden`.
fn always_hidden_title_for(divider: &str) -> Option<String> {
    divider
        .strip_suffix(".Hidden")
        .map(|prefix| format!("{prefix}.AlwaysHidden"))
}

/// Everything the probe found, as plain data.
#[derive(Debug, Default)]
struct Report {
    screen_recording: bool,
    post_events: bool,
    accessibility: bool,
    primary_height: f64,
    displays: Vec<(u32, Rect)>,
    status_windows: usize,
    active_display: Option<u32>,
    menu_bar_height: Option<f64>,
    height_from_backdrop: bool,
    divider_title: String,
    icon_title: String,
    divider: Option<WindowRecord>,
    divider_error: Option<String>,
    icon: Option<WindowRecord>,
    always_hidden_title: Option<String>,
    always_hidden_divider: Option<WindowRecord>,
    hidden: Vec<WindowRecord>,
    always_hidden: Vec<WindowRecord>,
    visible: Vec<WindowRecord>,
    captures: Vec<CaptureRow>,
    /// Set when the bounded capture did not finish (G5).
    capture_error: Option<String>,
    out_dir: Option<PathBuf>,
    /// macOS 27: MenuBarAgent's slots, left to right; `None` before macOS
    /// 27 or without an answer (v1.8).
    agent_slots: Option<Vec<AgentSlot>>,
}

impl Report {
    /// The system overflow control's frame, when MenuBarAgent shows one.
    fn overflow_control(&self) -> Option<&Rect> {
        self.agent_slots
            .as_ref()?
            .iter()
            .find(|s| s.overflow_control)
            .map(|s| &s.frame)
    }
}

/// Capture row error when the window server returned no image.
const CAPTURE_FAILED: &str = "capture failed: no image, no PNG written";
/// Probe error when the capture worker did not finish in time (G5).
const CAPTURE_TIMED_OUT: &str = "capture timed out";
/// Probe error when the capture worker could not start or panicked.
const CAPTURE_WORKER_FAILED: &str = "capture worker failed";

/// The report-level capture error for a bounded capture that failed.
fn capture_error_text(err: BoundedError) -> &'static str {
    match err {
        BoundedError::TimedOut | BoundedError::Busy => CAPTURE_TIMED_OUT,
        BoundedError::Failed => CAPTURE_WORKER_FAILED,
    }
}

#[derive(Debug, Default, Clone)]
struct CaptureRow {
    id: u32,
    method: &'static str,
    width_px: usize,
    height_px: usize,
    transparent: bool,
    file: Option<String>,
    error: Option<String>,
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_num(v: f64) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".into()
    }
}

fn json_opt_str(v: Option<&str>) -> String {
    v.map_or_else(|| "null".into(), json_str)
}

fn json_window(w: &WindowRecord) -> String {
    format!(
        "{{\"id\":{},\"title\":{},\"x\":{},\"y\":{},\"width\":{},\"height\":{},\"onscreen\":{}}}",
        w.id,
        json_opt_str(w.title.as_deref()),
        json_num(w.bounds.x),
        json_num(w.bounds.y),
        json_num(w.bounds.width),
        json_num(w.bounds.height),
        w.on_screen
    )
}

fn json_list<T>(items: &[T], f: impl Fn(&T) -> String) -> String {
    format!("[{}]", items.iter().map(f).collect::<Vec<_>>().join(","))
}

impl Report {
    fn to_json(&self) -> String {
        let displays = json_list(&self.displays, |(id, b)| {
            format!(
                "{{\"id\":{id},\"x\":{},\"y\":{},\"width\":{},\"height\":{},\"active\":{}}}",
                json_num(b.x),
                json_num(b.y),
                json_num(b.width),
                json_num(b.height),
                self.active_display == Some(*id)
            )
        });
        let captures = json_list(&self.captures, |c| {
            format!(
                "{{\"id\":{},\"method\":{},\"width_px\":{},\"height_px\":{},\"transparent\":{},\"file\":{},\"error\":{}}}",
                c.id,
                json_str(c.method),
                c.width_px,
                c.height_px,
                c.transparent,
                json_opt_str(c.file.as_deref()),
                json_opt_str(c.error.as_deref())
            )
        });
        let out_dir = self.out_dir.as_ref().map(|p| p.display().to_string());
        let overflow = self.overflow_control();
        let agent_slots = self.agent_slots.as_ref().map_or_else(
            || "null".into(),
            |slots| {
                json_list(slots, |s| {
                    format!(
                        "{{\"x\":{},\"y\":{},\"width\":{},\"height\":{},\"pid\":{},\"name\":{},\"overflow_control\":{},\"folded\":{}}}",
                        json_num(s.frame.x),
                        json_num(s.frame.y),
                        json_num(s.frame.width),
                        json_num(s.frame.height),
                        s.pid.map_or_else(|| "null".into(), |p| p.to_string()),
                        json_opt_str(s.name.as_deref()),
                        s.overflow_control,
                        !s.overflow_control && is_folded(&s.frame, overflow)
                    )
                })
            },
        );
        format!(
            concat!(
                "{{\"permissions\":{{\"screen_recording\":{},\"post_events\":{},\"accessibility\":{}}},",
                "\"primary_height\":{},\"displays\":{},\"status_windows\":{},",
                "\"active_display\":{},\"menu_bar_height\":{},\"menu_bar_height_source\":{},",
                "\"divider_title\":{},\"icon_title\":{},\"divider\":{},\"divider_error\":{},\"icon\":{},",
                "\"always_hidden_title\":{},\"always_hidden_divider\":{},",
                "\"hidden\":{},\"always_hidden\":{},\"visible\":{},\"captures\":{},",
                "\"capture_error\":{},\"out_dir\":{},\"agent_slots\":{}}}"
            ),
            self.screen_recording,
            self.post_events,
            self.accessibility,
            json_num(self.primary_height),
            displays,
            self.status_windows,
            self.active_display
                .map_or_else(|| "null".into(), |id| id.to_string()),
            self.menu_bar_height.map_or_else(|| "null".into(), json_num),
            json_opt_str(self.menu_bar_height_source()),
            json_str(&self.divider_title),
            json_str(&self.icon_title),
            self.divider
                .as_ref()
                .map_or_else(|| "null".into(), json_window),
            json_opt_str(self.divider_error.as_deref()),
            self.icon
                .as_ref()
                .map_or_else(|| "null".into(), json_window),
            json_opt_str(self.always_hidden_title.as_deref()),
            self.always_hidden_divider
                .as_ref()
                .map_or_else(|| "null".into(), json_window),
            json_list(&self.hidden, json_window),
            json_list(&self.always_hidden, json_window),
            json_list(&self.visible, json_window),
            captures,
            json_opt_str(self.capture_error.as_deref()),
            json_opt_str(out_dir.as_deref()),
            agent_slots,
        )
    }

    /// Where the menu bar height came from, or `None` without a height.
    fn menu_bar_height_source(&self) -> Option<&'static str> {
        self.menu_bar_height.map(|_| {
            if self.height_from_backdrop {
                "Menubar window"
            } else {
                "status window"
            }
        })
    }

    fn to_text(&self) -> String {
        let mut s = String::new();
        let yes = |b: bool| if b { "granted" } else { "missing" };
        let _ = writeln!(s, "ice-rust probe (read-only)");
        let _ = writeln!(s, "permissions");
        let _ = writeln!(s, "  screen recording  {}", yes(self.screen_recording));
        let _ = writeln!(s, "  post events       {}", yes(self.post_events));
        let _ = writeln!(s, "  accessibility     {}", yes(self.accessibility));
        let _ = writeln!(s, "displays (primary height {})", self.primary_height);
        for (id, b) in &self.displays {
            let mark = if self.active_display == Some(*id) {
                "  <- active menu bar"
            } else {
                ""
            };
            let _ = writeln!(
                s,
                "  id {id:<4} x {} y {} w {} h {}{mark}",
                b.x, b.y, b.width, b.height
            );
        }
        match (
            self.active_display,
            self.menu_bar_height,
            self.menu_bar_height_source(),
        ) {
            (Some(id), Some(h), Some(source)) => {
                let _ = writeln!(s, "active display  id {id}, menu bar height {h} ({source})");
            }
            _ => {
                let _ = writeln!(s, "active display  unknown");
            }
        }
        let _ = writeln!(s, "status windows (layer 25)  {}", self.status_windows);
        let window_line = |w: &WindowRecord| {
            format!(
                "id {} x {} y {} w {} h {} onscreen {}",
                w.id, w.bounds.x, w.bounds.y, w.bounds.width, w.bounds.height, w.on_screen
            )
        };
        match (&self.divider, &self.divider_error) {
            (Some(d), _) => {
                let _ = writeln!(s, "divider  {}  {}", self.divider_title, window_line(d));
            }
            (None, Some(err)) => {
                let _ = writeln!(s, "divider  {}  NOT FOUND: {err}", self.divider_title);
            }
            (None, None) => {
                let _ = writeln!(s, "divider  {}  NOT FOUND", self.divider_title);
            }
        }
        match &self.icon {
            Some(i) => {
                let _ = writeln!(s, "icon     {}  {}", self.icon_title, window_line(i));
            }
            None => {
                let _ = writeln!(s, "icon     {}  not found", self.icon_title);
            }
        }
        if let Some(title) = &self.always_hidden_title {
            match &self.always_hidden_divider {
                Some(d) => {
                    let _ = writeln!(s, "always   {title}  {}", window_line(d));
                }
                None => {
                    let _ = writeln!(s, "always   {title}  not found");
                }
            }
        }
        for (name, chain) in [
            ("hidden", &self.hidden),
            ("always-hidden", &self.always_hidden),
            ("visible", &self.visible),
        ] {
            let _ = writeln!(s, "{name} chain ({}, left to right)", chain.len());
            if !chain.is_empty() {
                let _ = writeln!(
                    s,
                    "  {:<3} {:<7} {:<36} {:>9} {:>6}  onscreen",
                    "#", "id", "title", "x", "width"
                );
            }
            for (i, w) in chain.iter().enumerate() {
                let _ = writeln!(
                    s,
                    "  {:<3} {:<7} {:<36} {:>9} {:>6}  {}",
                    i,
                    w.id,
                    w.title_str().unwrap_or("-"),
                    w.bounds.x,
                    w.bounds.width,
                    w.on_screen
                );
            }
        }
        let transparent: Vec<String> = self
            .captures
            .iter()
            .filter(|c| c.transparent)
            .map(|c| c.id.to_string())
            .collect();
        let dest = self.out_dir.as_ref().map_or_else(
            || "not written (no --out)".to_string(),
            |p| p.display().to_string(),
        );
        let _ = writeln!(s, "captures -> {dest}");
        if let Some(err) = &self.capture_error {
            let _ = writeln!(s, "  {err}");
        }
        for (i, c) in self.captures.iter().enumerate() {
            let _ = writeln!(
                s,
                "  {:<3} id {:<7} {:<9} {:>4}x{:<4} transparent {:<3} {}{}",
                i,
                c.id,
                c.method,
                c.width_px,
                c.height_px,
                if c.transparent { "yes" } else { "no" },
                c.file.as_deref().unwrap_or(""),
                c.error
                    .as_deref()
                    .map(|e| format!(" error: {e}"))
                    .unwrap_or_default()
            );
        }
        let _ = writeln!(
            s,
            "summary  {} hidden, {} always-hidden, {} visible, {} transparent{}",
            self.hidden.len(),
            self.always_hidden.len(),
            self.visible.len(),
            transparent.len(),
            if transparent.is_empty() {
                String::new()
            } else {
                format!(" ({})", transparent.join(", "))
            }
        );
        if let Some(slots) = &self.agent_slots {
            let overflow = self.overflow_control();
            let _ = writeln!(s, "MenuBarAgent slots (macOS 27)  {}", slots.len());
            for slot in slots {
                let kind = if slot.overflow_control {
                    "overflow control"
                } else if is_folded(&slot.frame, overflow) {
                    "folded"
                } else {
                    "on the bar"
                };
                let _ = writeln!(
                    s,
                    "  x {:>7} y {:>6} w {:>6}  pid {:<6} {:<16} {}",
                    slot.frame.x,
                    slot.frame.y,
                    slot.frame.width,
                    slot.pid.map_or_else(|| "-".into(), |p| p.to_string()),
                    kind,
                    slot.name.as_deref().unwrap_or("-")
                );
            }
        }
        s
    }
}

#[cfg(target_os = "macos")]
mod probe {
    use std::fs;

    use ice_rust::chain::{self, DividerLookup};
    use ice_rust::geometry;
    use ice_rust::model::STATUS_LAYER;
    use ice_rust::platform::capture::{self, CaptureMethod};
    use ice_rust::platform::{permissions, windows};

    use super::{
        CAPTURE_FAILED, CAPTURE_TIMED_OUT, CaptureRow, ProbeOptions, Report,
        always_hidden_title_for, capture_error_text, icon_title_for,
    };

    /// macOS 27: every slot on the bar cropped from MenuBarAgent's menu bar
    /// window, from the Window Server "Menubar" window and from the screen,
    /// as `agent-<index>-<source>.png`, to see which source draws the items.
    fn write_agent_captures(
        slots: &[ice_rust::agent_bar::AgentSlot],
        all: &[ice_rust::model::WindowRecord],
        displays: &[geometry::Display],
        dir: &std::path::Path,
    ) {
        use ice_rust::platform::capture::{SlotSource, capture_slots, is_transparent};
        let overflow = slots.iter().find(|s| s.overflow_control).map(|s| s.frame);
        for display in displays {
            let d = display.bounds;
            let on_display = |r: &ice_rust::model::Rect| {
                r.min_x() >= d.min_x() && r.max_x() <= d.max_x() && (r.y - d.y).abs() <= 1.0
            };
            let Some(agent) = all.iter().find(|w| {
                w.owner_name.as_deref() == Some(ice_rust::model::MENU_BAR_AGENT_OWNER)
                    && on_display(&w.bounds)
            }) else {
                continue;
            };
            let region = agent.bounds;
            let rects: Vec<_> = slots
                .iter()
                .filter(|s| {
                    !s.overflow_control
                        && on_display(&s.frame)
                        && !ice_rust::agent_bar::is_folded(&s.frame, overflow.as_ref())
                })
                .map(|s| s.frame)
                .collect();
            let mut sources = vec![("agent", SlotSource::Window(agent.id))];
            if let Some(menubar) = geometry::menu_bar_window(all, &d) {
                sources.push(("menubar", SlotSource::Window(menubar.id)));
            }
            sources.push(("screen", SlotSource::Screen));
            for (label, source) in sources {
                let images = capture_slots(source, &region, &rects);
                let mut written = 0;
                let mut clear = 0;
                for (index, image) in images.iter().enumerate() {
                    let Some(image) = image else {
                        continue;
                    };
                    if is_transparent(image) {
                        clear += 1;
                    }
                    let name = format!("agent-{index:02}-{label}.png");
                    if capture::write_png(image, &dir.join(name)).is_ok() {
                        written += 1;
                    }
                }
                eprintln!(
                    "agent captures from {label} ({}): {written} of {} written, {clear} transparent",
                    source.label(),
                    rects.len()
                );
            }
        }
    }

    /// Read-only: window list queries, captures, and file writes into
    /// `--out` only.
    pub fn run(options: &ProbeOptions) -> (Report, bool) {
        let mut report = Report {
            divider_title: options.divider.clone(),
            icon_title: icon_title_for(&options.divider),
            always_hidden_title: always_hidden_title_for(&options.divider),
            out_dir: options.out.clone(),
            ..Report::default()
        };

        let granted = permissions::check();
        report.screen_recording = granted.screen_recording;
        report.post_events = granted.post_events;
        report.accessibility = granted.accessibility;

        let all = windows::list_windows();
        let displays = windows::displays();
        report.primary_height = windows::primary_height();
        report.displays = displays.iter().map(|d| (d.id, d.bounds)).collect();
        report.status_windows = all.iter().filter(|w| w.layer == STATUS_LAYER).count();
        if ice_rust::agent_bar::uses_agent_menu_bar(ice_rust::platform::status::os_major()) {
            report.agent_slots =
                ice_rust::platform::menu_bar_space::agent_dump().map(|mut slots| {
                    slots.sort_by(|a, b| a.frame.x.total_cmp(&b.frame.x));
                    slots
                });
            if let (Some(slots), Some(dir)) = (&report.agent_slots, &options.out)
                && fs::create_dir_all(dir).is_ok()
            {
                write_agent_captures(slots, &all, &displays, dir);
            }
        }

        report.icon = chain::find_titled(&all, &report.icon_title).cloned();
        let divider = match chain::find_divider(&all, &report.divider_title, &report.icon_title) {
            DividerLookup::Found(d) => Some(d),
            DividerLookup::NotFound => {
                report.divider_error = Some("no status window has this title".into());
                None
            }
            DividerLookup::TitlesUnreadable => {
                report.divider_error =
                    Some("window titles are unreadable; Screen Recording is missing".into());
                None
            }
            DividerLookup::NoItemWindows => {
                report.divider_error =
                    Some("no status item windows; the menu bar draws items without windows".into());
                None
            }
        };

        let icon = report.icon.as_ref().map(|w| &w.bounds);
        if let Some(place) = geometry::menu_bar_display(
            &all,
            &displays,
            icon,
            divider.as_ref().map(|d| &d.bounds),
            None,
        ) {
            report.active_display = Some(place.display.id);
            report.menu_bar_height = Some(place.menu_bar_height);
            report.height_from_backdrop = place.height_from_backdrop;
        }

        let Some(divider) = divider else {
            return (report, false);
        };
        report.always_hidden_divider = report.always_hidden_title.as_deref().and_then(|title| {
            match chain::find_divider(&all, title, &report.icon_title) {
                DividerLookup::Found(d) => Some(d),
                _ => None,
            }
        });
        let chains = chain::section_chains(&all, &divider, report.always_hidden_divider.as_ref());
        report.divider = Some(divider);
        report.hidden = chains.hidden;
        report.always_hidden = chains.always_hidden;
        report.visible = chains.visible;

        let mut out_error = None;
        if let Some(dir) = &options.out
            && let Err(err) = fs::create_dir_all(dir)
        {
            out_error = Some(format!("cannot create {}: {err}", dir.display()));
        }

        // Bounded (G5): a capture stuck in the window server proxy ends the
        // wait after CAPTURE_TIMEOUT and leaves its thread behind.
        let result = capture::capture_items_bounded(&report.hidden);
        report.capture_error = result
            .as_ref()
            .err()
            .map(|err| capture_error_text(*err).to_string());
        let images = capture::images_or_placeholders(&report.hidden, result);
        if report.capture_error.is_none()
            && images.iter().any(|i| i.method == CaptureMethod::TimedOut)
        {
            // The worker reached its deadline before capturing every item.
            report.capture_error = Some(CAPTURE_TIMED_OUT.into());
        }
        for (index, item) in images.iter().enumerate() {
            let (width_px, height_px) = item.pixel_size();
            let mut row = CaptureRow {
                id: item.id,
                method: item.method.as_str(),
                width_px,
                height_px,
                transparent: item.transparent,
                file: None,
                error: out_error.clone(),
            };
            if item.method == CaptureMethod::TimedOut {
                row.error = Some(CAPTURE_TIMED_OUT.into());
            } else if let (Some(dir), None) = (&options.out, &out_error) {
                match item.image.as_deref() {
                    Some(image) => {
                        let name = format!("{index:02}-{}.png", item.id);
                        match capture::write_png(image, &dir.join(&name)) {
                            Ok(()) => row.file = Some(name),
                            Err(err) => row.error = Some(err.to_string()),
                        }
                    }
                    None => row.error = Some(CAPTURE_FAILED.into()),
                }
            }
            report.captures.push(row);
        }
        (report, true)
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse_args(&args) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("ice-rust: {msg}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match command {
        Command::Help => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Command::Gui => run_gui(),
        Command::Probe(options) => run_probe(&options),
        Command::Render(args) => run_render(&args),
        Command::Update(command) => run_update(&command),
    }
}

#[cfg(target_os = "macos")]
fn run_update(command: &update_cli::UpdateCommand) -> ExitCode {
    update_cli::run::run(command)
}

#[cfg(not(target_os = "macos"))]
fn run_update(_command: &update_cli::UpdateCommand) -> ExitCode {
    eprintln!("ice-rust: updates are available on macOS only");
    ExitCode::FAILURE
}

#[cfg(target_os = "macos")]
fn run_render(args: &RenderArgs) -> ExitCode {
    use ice_rust::platform::render::{RenderOptions, render};
    use ice_rust::platform::settings_window::Page;

    let Some(page) = Page::from_arg(&args.page) else {
        eprintln!("ice-rust: unknown page {}", args.page);
        return ExitCode::from(2);
    };
    let options = RenderOptions {
        page,
        out: args.out.clone(),
        size: (args.width, args.height),
        prefix: args.prefix.clone(),
        default_settings: args.default_settings,
    };
    match render(&options) {
        Ok(report) => {
            println!(
                "rendered {} page, {}x{} px -> {}",
                args.page,
                report.pixels.0,
                report.pixels.1,
                args.out.display()
            );
            if page == Page::Layout {
                println!(
                    "dividers  {}",
                    report.prefix.as_deref().unwrap_or("not found")
                );
                if let Some(notice) = &report.notice {
                    println!("notice    {notice}");
                }
                for (section, tiles, with_image, timed_out) in &report.sections {
                    println!(
                        "{:<22} {tiles} tiles, {with_image} with images{}",
                        section.label(),
                        if *timed_out {
                            ", capture timed out"
                        } else {
                            ""
                        }
                    );
                }
                match report.bar_color {
                    Some([r, g, b]) => println!("bar color rgb({r:.3}, {g:.3}, {b:.3})"),
                    None => println!("bar color fallback (sample unavailable)"),
                }
                for (section, offset, max) in &report.scroll {
                    println!(
                        "{:<22} scrolled to {offset:.1} of {max:.1}",
                        section.label()
                    );
                }
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("ice-rust: render-settings failed: {err}");
            ExitCode::from(1)
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn run_render(_args: &RenderArgs) -> ExitCode {
    eprintln!("ice-rust: render-settings needs macOS");
    ExitCode::from(1)
}

#[cfg(target_os = "macos")]
fn run_gui() -> ExitCode {
    ice_rust::platform::app::run();
    ExitCode::SUCCESS
}

#[cfg(target_os = "macos")]
fn run_probe(options: &ProbeOptions) -> ExitCode {
    let (report, found) = probe::run(options);
    if options.json {
        println!("{}", report.to_json());
    } else {
        print!("{}", report.to_text());
    }
    if found {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

#[cfg(not(target_os = "macos"))]
fn run_gui() -> ExitCode {
    eprintln!("ice-rust: the menu bar app needs macOS");
    ExitCode::from(1)
}

#[cfg(not(target_os = "macos"))]
fn run_probe(_options: &ProbeOptions) -> ExitCode {
    eprintln!("ice-rust: probe needs macOS");
    ExitCode::from(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ice_rust::model::STATUS_LAYER;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_args_runs_gui() {
        assert_eq!(parse_args(&[]), Ok(Command::Gui));
    }

    #[test]
    fn probe_defaults() {
        assert_eq!(
            parse_args(&args(&["probe"])),
            Ok(Command::Probe(ProbeOptions {
                divider: DIVIDER_AUTOSAVE.to_string(),
                out: None,
                json: false
            }))
        );
    }

    #[test]
    fn probe_all_flags() {
        assert_eq!(
            parse_args(&args(&[
                "probe",
                "--json",
                "--divider",
                "Ice.ControlItem.Hidden",
                "--out",
                "/tmp/x"
            ])),
            Ok(Command::Probe(ProbeOptions {
                divider: "Ice.ControlItem.Hidden".into(),
                out: Some(PathBuf::from("/tmp/x")),
                json: true
            }))
        );
    }

    #[test]
    fn probe_rejects_bad_args() {
        assert!(parse_args(&args(&["probe", "--divider"])).is_err());
        assert!(parse_args(&args(&["probe", "--divider", ""])).is_err());
        assert!(parse_args(&args(&["probe", "--out"])).is_err());
        assert!(parse_args(&args(&["probe", "--bogus"])).is_err());
        assert!(parse_args(&args(&["bogus"])).is_err());
    }

    #[test]
    fn help_flags() {
        assert_eq!(parse_args(&args(&["--help"])), Ok(Command::Help));
        assert_eq!(parse_args(&args(&["probe", "-h"])), Ok(Command::Help));
    }

    #[test]
    fn render_settings_args() {
        assert_eq!(
            parse_args(&args(&[
                "render-settings",
                "--page",
                "layout",
                "--out",
                "/tmp/a.png"
            ])),
            Ok(Command::Render(RenderArgs {
                page: "layout".into(),
                out: PathBuf::from("/tmp/a.png"),
                width: 820.0,
                height: 600.0,
                prefix: None,
                default_settings: false,
            }))
        );
        assert_eq!(
            parse_args(&args(&[
                "render-settings",
                "--out",
                "x.png",
                "--page",
                "about",
                "--width",
                "900",
                "--height",
                "640",
                "--divider-prefix",
                "Ice.ControlItem.",
                "--default-settings"
            ])),
            Ok(Command::Render(RenderArgs {
                page: "about".into(),
                out: PathBuf::from("x.png"),
                width: 900.0,
                height: 640.0,
                prefix: Some("Ice.ControlItem.".into()),
                default_settings: true,
            }))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn every_page_name_opens_its_page() {
        use ice_rust::platform::settings_window::Page;
        assert_eq!(PAGES.len(), Page::ALL.len());
        for (name, page) in PAGES.iter().zip(Page::ALL) {
            assert_eq!(Page::from_arg(name), Some(page), "{name}");
        }
    }

    #[test]
    fn render_settings_rejects_bad_args() {
        for bad in [
            vec!["render-settings"],
            vec!["render-settings", "--page", "layout"],
            vec!["render-settings", "--out", "a.png"],
            vec!["render-settings", "--page", "hotkeys", "--out", "a.png"],
            vec![
                "render-settings",
                "--page",
                "general",
                "--out",
                "a.png",
                "--width",
                "abc",
            ],
            vec![
                "render-settings",
                "--page",
                "general",
                "--out",
                "a.png",
                "--height",
                "50",
            ],
            vec![
                "render-settings",
                "--page",
                "general",
                "--out",
                "a.png",
                "--divider-prefix",
                "",
            ],
            vec![
                "render-settings",
                "--page",
                "general",
                "--out",
                "a.png",
                "--bogus",
            ],
        ] {
            assert!(parse_args(&args(&bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn always_hidden_title_pairs_with_divider() {
        assert_eq!(
            always_hidden_title_for("Ice.ControlItem.Hidden").as_deref(),
            Some("Ice.ControlItem.AlwaysHidden")
        );
        assert_eq!(
            always_hidden_title_for(DIVIDER_AUTOSAVE).as_deref(),
            Some("IceRust.ControlItem.AlwaysHidden")
        );
        assert_eq!(always_hidden_title_for("Something.Else"), None);
    }

    #[test]
    fn timed_out_capture_is_reported() {
        let report = Report {
            capture_error: Some(CAPTURE_TIMED_OUT.into()),
            captures: vec![CaptureRow {
                id: 52,
                method: "timed out",
                transparent: true,
                error: Some(CAPTURE_TIMED_OUT.into()),
                ..CaptureRow::default()
            }],
            ..Report::default()
        };
        assert!(report.to_text().contains("  capture timed out\n"));
        assert!(
            report
                .to_json()
                .contains("\"capture_error\":\"capture timed out\"")
        );
        assert!(report.to_json().contains("\"method\":\"timed out\""));
    }

    #[test]
    fn capture_errors_name_their_cause() {
        assert_eq!(
            capture_error_text(BoundedError::TimedOut),
            CAPTURE_TIMED_OUT
        );
        assert_eq!(capture_error_text(BoundedError::Busy), CAPTURE_TIMED_OUT);
        assert_eq!(
            capture_error_text(BoundedError::Failed),
            "capture worker failed"
        );
    }

    #[test]
    fn icon_title_pairs_with_divider() {
        assert_eq!(
            icon_title_for("Ice.ControlItem.Hidden"),
            "Ice.ControlItem.Visible"
        );
        assert_eq!(icon_title_for(DIVIDER_AUTOSAVE), ICON_AUTOSAVE);
        assert_eq!(icon_title_for("Something.Else"), ICON_AUTOSAVE);
    }

    #[test]
    fn json_escaping() {
        assert_eq!(json_str("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
        assert_eq!(json_num(-394.0), "-394");
        assert_eq!(json_num(1.5), "1.5");
        assert_eq!(json_num(f64::NAN), "null");
    }

    #[test]
    fn report_json_shape() {
        let w = WindowRecord {
            id: 312,
            layer: STATUS_LAYER,
            bounds: Rect::new(-498.0, 0.0, 32.0, 30.0),
            title: Some("Item-0".into()),
            owner_name: None,
            on_screen: false,
        };
        let report = Report {
            screen_recording: true,
            displays: vec![(5, Rect::new(2560.0, 0.0, 2560.0, 1440.0))],
            active_display: Some(5),
            menu_bar_height: Some(30.0),
            height_from_backdrop: true,
            divider_title: "Ice.ControlItem.Hidden".into(),
            icon_title: "Ice.ControlItem.Visible".into(),
            hidden: vec![w],
            captures: vec![CaptureRow {
                id: 312,
                method: "composite",
                width_px: 64,
                height_px: 60,
                transparent: false,
                file: Some("00-312.png".into()),
                error: None,
            }],
            ..Report::default()
        };
        let json = report.to_json();
        assert!(json.starts_with("{\"permissions\":{\"screen_recording\":true"));
        assert!(json.contains("\"displays\":[{\"id\":5,\"x\":2560,\"y\":0,\"width\":2560,\"height\":1440,\"active\":true}]"));
        assert!(json.contains("\"hidden\":[{\"id\":312,\"title\":\"Item-0\",\"x\":-498,\"y\":0,\"width\":32,\"height\":30,\"onscreen\":false}]"));
        assert!(json.contains("\"file\":\"00-312.png\""));
        assert!(json.contains("\"divider\":null"));
        assert!(json.ends_with("\"out_dir\":null,\"agent_slots\":null}"));
        assert!(
            json.contains("\"menu_bar_height\":30,\"menu_bar_height_source\":\"Menubar window\"")
        );
        let text = report.to_text();
        assert!(text.contains("hidden chain (1, left to right)"));
        assert!(text.contains("summary  1 hidden, 0 always-hidden, 0 visible, 0 transparent"));
        assert!(json.contains("\"always_hidden\":[]"));

        // macOS 27: MenuBarAgent's slots, folded ones left of the overflow
        // control (VM evidence).
        let agent = Report {
            agent_slots: Some(vec![
                AgentSlot {
                    frame: Rect::new(401.0, 0.0, 44.0, 30.0),
                    pid: Some(1235),
                    name: Some("D15".into()),
                    identifier: None,
                    overflow_control: false,
                },
                AgentSlot {
                    frame: Rect::new(428.0, 1.0, 18.0, 27.0),
                    pid: Some(419),
                    name: Some("Show Hidden Menu Bar Items".into()),
                    identifier: None,
                    overflow_control: true,
                },
                AgentSlot {
                    frame: Rect::new(453.0, 0.0, 38.0, 30.0),
                    pid: Some(1235),
                    name: Some("D8".into()),
                    identifier: None,
                    overflow_control: false,
                },
            ]),
            ..Report::default()
        };
        let json = agent.to_json();
        assert!(json.contains(
            "{\"x\":401,\"y\":0,\"width\":44,\"height\":30,\"pid\":1235,\"name\":\"D15\",\"overflow_control\":false,\"folded\":true}"
        ));
        assert!(json.contains("\"name\":\"D8\",\"overflow_control\":false,\"folded\":false}"));
        let text = agent.to_text();
        assert!(text.contains("MenuBarAgent slots (macOS 27)  3"));
        assert!(text.contains("folded           D15"));
        assert!(text.contains("overflow control Show Hidden Menu Bar Items"));
        assert!(json.contains("\"capture_error\":null"));
    }

    #[test]
    fn height_source_is_null_without_height() {
        let report = Report::default();
        assert!(
            report
                .to_json()
                .contains("\"menu_bar_height\":null,\"menu_bar_height_source\":null")
        );
        assert!(report.to_text().contains("active display  unknown"));
        let report = Report {
            active_display: Some(5),
            menu_bar_height: Some(30.0),
            ..Report::default()
        };
        assert!(
            report
                .to_json()
                .contains("\"menu_bar_height_source\":\"status window\"")
        );
        assert!(
            report
                .to_text()
                .contains("menu bar height 30 (status window)")
        );
    }

    #[test]
    fn failed_capture_row_reports_error() {
        let report = Report {
            captures: vec![CaptureRow {
                id: 52,
                method: "failed",
                transparent: true,
                error: Some(CAPTURE_FAILED.into()),
                ..CaptureRow::default()
            }],
            ..Report::default()
        };
        assert!(
            report
                .to_json()
                .contains("\"file\":null,\"error\":\"capture failed: no image, no PNG written\"")
        );
        assert!(report.to_text().contains("error: capture failed"));
    }
}
