use std::cmp::Ordering;
use std::ffi::c_char;
use std::ffi::CStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::prelude::*;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table};
// ───────────────────────────── FFI ─────────────────────────────
#[repr(C)]
struct SensorEntry {
    chip: *const c_char,
    label: *const c_char,
    type_: i32,
    input: f64,
    has_min: i32,
    min: f64,
    has_max: i32,
    max: f64,
    has_lcrit: i32,
    lcrit: f64,
    has_crit: i32,
    crit: f64,
    alarm: i32,
}

unsafe extern "C" {
    fn rs_sensors_init(err: *mut *mut c_char) -> i32;
    fn rs_sensors_cleanup();
    fn rs_sensors_refresh(count: *mut i32);
    fn rs_sensors_entry(idx: i32) -> *const SensorEntry;
    fn rs_sensors_version() -> *const c_char;
}

unsafe fn cstr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
}

// ───────────────────────────── Data model ─────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq)]
enum SensorKind {
    Temp,
    Fan,
    Voltage,
    Power,
    Energy,
    Current,
    Humidity,
    Other,
}

impl SensorKind {
    const fn from_type(t: i32) -> SensorKind {
        match t {
            0x00 => SensorKind::Voltage,
            0x01 => SensorKind::Fan,
            0x02 => SensorKind::Temp,
            0x03 => SensorKind::Power,
            0x04 => SensorKind::Energy,
            0x05 => SensorKind::Current,
            0x06 => SensorKind::Humidity,
            _ => SensorKind::Other,
        }
    }
    const fn label(&self) -> &'static str {
        match self {
            SensorKind::Temp => "TEMP",
            SensorKind::Fan => "FAN",
            SensorKind::Voltage => "VOLT",
            SensorKind::Power => "PWR",
            SensorKind::Energy => "ENERGY",
            SensorKind::Current => "CURR",
            SensorKind::Humidity => "HUM",
            SensorKind::Other => "OTHER",
        }
    }
    const fn order(&self) -> i32 {
        match self {
            SensorKind::Temp => 0,
            SensorKind::Fan => 1,
            SensorKind::Power => 2,
            SensorKind::Voltage => 3,
            SensorKind::Current => 4,
            SensorKind::Energy => 5,
            SensorKind::Humidity => 6,
            SensorKind::Other => 7,
        }
    }
}

#[derive(Clone)]
struct Sensor {
    chip: String,
    label: String,
    kind: SensorKind,
    value: f64,
    min: Option<f64>,
    max: Option<f64>,
    lcrit: Option<f64>,
    crit: Option<f64>,
    alarm: bool,
}

// Owns the libsensors handle and the current snapshot of sensors.
struct Sensors {
    entries: Vec<Sensor>,
}

impl Sensors {
    fn new() -> Result<Self> {
        let mut err: *mut c_char = std::ptr::null_mut();
        let rc = unsafe { rs_sensors_init(&mut err) };
        if rc != 0 {
            let msg = if err.is_null() {
                "sensors_init failed".to_string()
            } else {
                unsafe { cstr_to_string(err) }
            };
            return Err(anyhow::anyhow!("failed to init libsensors: {}", msg));
        }
        let mut s = Self {
            entries: Vec::new(),
        };
        s.refresh()?;
        Ok(s)
    }

    fn refresh(&mut self) -> Result<()> {
        let mut count: i32 = 0;
        unsafe {
            rs_sensors_refresh(&mut count);
        }
        self.entries.clear();
        for i in 0..count {
            unsafe {
                let e = rs_sensors_entry(i);
                if e.is_null() {
                    continue;
                }
                self.entries.push(Sensor {
                    chip: cstr_to_string((*e).chip),
                    label: cstr_to_string((*e).label),
                    kind: SensorKind::from_type((*e).type_),
                    value: (*e).input,
                    min: if (*e).has_min == 1 {
                        Some((*e).min)
                    } else {
                        None
                    },
                    max: if (*e).has_max == 1 {
                        Some((*e).max)
                    } else {
                        None
                    },
                    lcrit: if (*e).has_lcrit == 1 {
                        Some((*e).lcrit)
                    } else {
                        None
                    },
                    crit: if (*e).has_crit == 1 {
                        Some((*e).crit)
                    } else {
                        None
                    },
                    alarm: (*e).alarm != 0,
                });
            }
        }
        Ok(())
    }

    fn version() -> String {
        unsafe { cstr_to_string(rs_sensors_version()) }
    }
}

impl Drop for Sensors {
    fn drop(&mut self) {
        unsafe {
            rs_sensors_cleanup();
        }
    }
}

// ───────────────────────────── Formatting ─────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    Warning,
    Alarm,
}

fn status_of(s: &Sensor) -> Status {
    if s.alarm {
        return Status::Alarm;
    }
    if s.value.is_nan() {
        return Status::Ok;
    }
    if s.kind == SensorKind::Temp {
        if s.value < 0.0 {
            return Status::Ok;
        }
        if let Some(crit) = s.crit {
            if s.value > crit {
                return Status::Alarm;
            }
        }
        if let Some(lcrit) = s.lcrit {
            if s.value < lcrit {
                return Status::Warning;
            }
        }
    }
    if let Some(max) = s.max {
        if s.value > max {
            return Status::Warning;
        }
    }
    if let Some(min) = s.min {
        if s.value < min {
            return Status::Warning;
        }
    }
    Status::Ok
}

fn fmt_value(kind: SensorKind, v: f64) -> String {
    if v.is_nan() {
        return "—".to_string();
    }
    match kind {
        SensorKind::Temp => {
            if v < 0.0 {
                "—".to_string()
            } else {
                format!("{:.1} °C", v)
            }
        }
        SensorKind::Fan => {
            if v < 1.0 {
                "fault".to_string()
            } else {
                format!("{:.0} RPM", v)
            }
        }
        SensorKind::Voltage => format!("{:.2} V", v),
        SensorKind::Power => {
            if v.abs() < 1.0 {
                format!("{:.1} mW", v * 1000.0)
            } else {
                format!("{:.2} W", v)
            }
        }
        SensorKind::Current => format!("{:.2} A", v),
        SensorKind::Energy => format!("{:.2} kWh", v),
        SensorKind::Humidity => format!("{:.0} %", v),
        SensorKind::Other => format!("{}", v),
    }
}

fn fmt_unit(kind: SensorKind, v: f64) -> String {
    if v.is_nan() {
        return "—".to_string();
    }
    match kind {
        SensorKind::Temp => {
            if v < 0.0 {
                "—".to_string()
            } else {
                format!("{:.1}°C", v)
            }
        }
        SensorKind::Fan => {
            if v < 1.0 {
                "fault".to_string()
            } else {
                format!("{:.0}RPM", v)
            }
        }
        SensorKind::Voltage => format!("{:.2}V", v),
        SensorKind::Power => {
            if v.abs() < 1.0 {
                format!("{:.1}mW", v * 1000.0)
            } else {
                format!("{:.2}W", v)
            }
        }
        SensorKind::Current => format!("{:.2}A", v),
        SensorKind::Energy => format!("{:.2}kWh", v),
        SensorKind::Humidity => format!("{:.0}%", v),
        SensorKind::Other => format!("{}", v),
    }
}

fn fmt_threshold(kind: SensorKind, v: Option<f64>) -> String {
    match v {
        Some(x) => fmt_unit(kind, x),
        None => "—".to_string(),
    }
}

fn temp_bar(s: &Sensor) -> String {
    if s.kind != SensorKind::Temp || s.value.is_nan() || s.value < 0.0 {
        return " ".repeat(10);
    }
    let lo = s.min.unwrap_or(0.0);
    let hi = s.max.unwrap_or(105.0);
    if hi <= lo {
        return " ".repeat(10);
    }
    let pos = ((s.value - lo) / (hi - lo)).clamp(0.0, 1.0);
    let width = 10;
    let filled = (pos * width as f64) as usize;
    (0..width)
        .map(|i| if i < filled { '█' } else { '▒' })
        .collect()
}

fn status_label(st: Status) -> &'static str {
    match st {
        Status::Alarm => "ALARM",
        Status::Warning => "WARN",
        Status::Ok => "OK",
    }
}

// ───────────────────────────── Filter ─────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Temp,
    Fan,
    Power,
    Voltage,
    Current,
    Energy,
}

impl Filter {
    const fn next(&self) -> Filter {
        match self {
            Filter::All => Filter::Temp,
            Filter::Temp => Filter::Fan,
            Filter::Fan => Filter::Power,
            Filter::Power => Filter::Voltage,
            Filter::Voltage => Filter::Current,
            Filter::Current => Filter::Energy,
            Filter::Energy => Filter::All,
        }
    }
    const fn label(&self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Temp => "Temp",
            Filter::Fan => "Fan",
            Filter::Power => "Power",
            Filter::Voltage => "Voltage",
            Filter::Current => "Current",
            Filter::Energy => "Energy",
        }
    }
    fn matches(&self, s: &Sensor) -> bool {
        match self {
            Filter::All => true,
            Filter::Temp => s.kind == SensorKind::Temp,
            Filter::Fan => s.kind == SensorKind::Fan,
            Filter::Power => s.kind == SensorKind::Power,
            Filter::Voltage => s.kind == SensorKind::Voltage,
            Filter::Current => s.kind == SensorKind::Current,
            Filter::Energy => s.kind == SensorKind::Energy,
        }
    }
}

// ───────────────────────────── UI state ─────────────────────────────
struct UiState {
    sensors: Vec<Sensor>,
    version: String,
    filter: Filter,
    sort_by_value: bool,
    show_help: bool,
    selected: Option<(String, String)>,
    last_update: SystemTime,
    force_refresh: bool,
}

fn move_selection(state: &mut UiState, dir: i32) {
    let visible = visible_sensors(state);
    let current = state.selected.as_ref().and_then(|id| {
        visible.iter().position(|s| s.chip == id.0 && s.label == id.1)
    });
    let next = match current {
        None => {
            if dir > 0 {
                Some(0)
            } else {
                visible.last().map(|_| visible.len() - 1)
            }
        }
        Some(i) => {
            let n = visible.len();
            if n == 0 {
                None
            } else {
                let next = (i as i32 + dir).rem_euclid(n as i32);
                Some(next as usize)
            }
        }
    };
    state.selected = match next {
        Some(i) => Some((visible[i].chip.clone(), visible[i].label.clone())),
        None => None,
    };
}

fn sort_label(by_value: bool) -> String {
    if by_value {
        "value (desc)".to_string()
    } else {
        "chip order".to_string()
    }
}

fn visible_sensors(state: &UiState) -> Vec<&Sensor> {
    let mut visible: Vec<&Sensor> =
        state.sensors.iter().filter(|s| state.filter.matches(s)).collect();
    if state.sort_by_value {
        visible.sort_by(|a, b| b.value.partial_cmp(&a.value).unwrap_or(Ordering::Less));
    } else {
        visible.sort_by(|a, b| {
            a.kind.order()
                .cmp(&b.kind.order())
                .then_with(|| a.chip.cmp(&b.chip))
                .then_with(|| a.label.cmp(&b.label))
        });
    }
    visible
}

fn build_rows(state: &UiState) -> Vec<Row<'_>> {
    let visible = visible_sensors(state);
    let sel_idx = state
        .selected
        .as_ref()
        .and_then(|id| visible.iter().position(|s| s.chip == id.0 && s.label == id.1));
    visible
        .iter()
        .enumerate()
        .map(|(i, s)| build_row(s, Some(i) == sel_idx))
        .collect()
}

fn build_row(s: &Sensor, selected: bool) -> Row<'static> {
    let st = status_of(s);
    let cell = |fg: Color| {
        if selected {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default().fg(fg)
        }
    };
    Row::new(vec![
        Cell::from(s.chip.clone()).style(cell(Color::White)),
        Cell::from(s.label.clone()).style(cell(Color::White)),
        Cell::from(s.kind.label()).style(cell(Color::Cyan)),
        Cell::from(fmt_value(s.kind, s.value)).style(cell(if st == Status::Alarm {
            Color::Red
        } else if st == Status::Warning {
            Color::Yellow
        } else {
            Color::Green
        })),
        Cell::from(temp_bar(s)).style(cell(Color::White)),
        Cell::from(fmt_threshold(s.kind, s.min)).style(cell(Color::DarkGray)),
        Cell::from(fmt_threshold(s.kind, s.max)).style(cell(Color::DarkGray)),
        Cell::from(fmt_threshold(s.kind, s.crit)).style(cell(Color::DarkGray)),
        Cell::from(status_label(st)).style(cell(if st == Status::Alarm {
            Color::Red
        } else if st == Status::Warning {
            Color::Yellow
        } else {
            Color::Green
        })),
    ])
}

fn format_time(t: &SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

// ───────────────────────────── Drawing ─────────────────────────────
fn draw_ui(f: &mut Frame, state: &UiState) {
    let area = f.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(0),
            Constraint::Length(3),
        ]);
    let rects = vertical.split(area);
    let header_area = rects[0];
    let table_area = rects[1];
    let footer_area = rects[2];

    let max_temp = state
        .sensors
        .iter()
        .filter(|s| s.kind == SensorKind::Temp && !s.value.is_nan())
        .map(|s| s.value)
        .max_by(f64::total_cmp)
        .unwrap_or(0.0);

    let header_title = "rs-sensors — TUI sensor monitor";
    let header_body = format!(
        "libsensors {} · {} sensors · max temp {} · updated {}",
        state.version,
        state.sensors.len(),
        fmt_unit(SensorKind::Temp, max_temp),
        format_time(&state.last_update),
    );
    let footer_body = format!(
        "{} | sort: {} | [q]uit [f]ilter [s]ort [r]efresh [?]help",
        state.filter.label(),
        sort_label(state.sort_by_value),
    );

    if state.show_help {
        let help = [
            "rs-sensors — TUI sensor monitor",
            "",
            "Keys:",
            "  q     quit",
            "  ↑/↓   move selection (also PgUp/PgDn, Home/End)",
            "  f     cycle filter (All/Temp/Fan/Power/Voltage/Current/Energy)",
            "  s     toggle sort (value desc / chip order)",
            "  r     force refresh now",
            "  ? or h  toggle help",
            "",
            "Sensors are read live via libsensors every second.",
        ]
        .join("\n");
        f.render_widget(
            Paragraph::new(help).block(
                Block::bordered()
                    .title("help")
                    .style(Style::default().fg(Color::Cyan)),
            ),
            table_area,
        );
        f.render_widget(
            Paragraph::new(header_body).block(
                Block::bordered()
                    .title(header_title)
                    .style(Style::default().fg(Color::Cyan).bold()),
            ),
            header_area,
        );
        f.render_widget(
            Paragraph::new(footer_body).block(
                Block::bordered()
                    .title("status")
                    .style(Style::default().fg(Color::DarkGray)),
            ),
            footer_area,
        );
        return;
    }

    f.render_widget(
        Paragraph::new(header_body).block(
            Block::bordered()
                .title(header_title)
                .style(Style::default().fg(Color::Cyan).bold()),
        ),
        header_area,
    );

    let rows = build_rows(state);
    let header_style = Style::default().fg(Color::Cyan).bold();
    let widths = vec![
        Constraint::Percentage(23),
        Constraint::Percentage(14),
        Constraint::Percentage(6),
        Constraint::Percentage(12),
        Constraint::Percentage(12),
        Constraint::Percentage(8),
        Constraint::Percentage(9),
        Constraint::Percentage(8),
        Constraint::Percentage(8),
    ];
    let table = Table::new(rows, widths)
        .header(Row::new(vec![
            Cell::from("Chip").style(header_style),
            Cell::from("Sensor").style(header_style),
            Cell::from("Type").style(header_style),
            Cell::from("Value").style(header_style),
            Cell::from("Bar").style(header_style),
            Cell::from("Min").style(header_style),
            Cell::from("Max").style(header_style),
            Cell::from("Crit").style(header_style),
            Cell::from("Status").style(header_style),
        ]))
        .style(Style::default().fg(Color::White));
    f.render_widget(table, table_area);

    f.render_widget(
        Paragraph::new(footer_body).block(
            Block::bordered()
                .title("status")
                .style(Style::default().fg(Color::DarkGray)),
        ),
        footer_area,
    );
}

// ───────────────────────────── Event handling ─────────────────────────────
fn handle_key(key: KeyEvent, state: &mut UiState, running: &mut bool) {
    match key.code {
        KeyCode::Char('q') => *running = false,
        KeyCode::Char('r') => state.force_refresh = true,
        KeyCode::Char('f') => state.filter = state.filter.next(),
        KeyCode::Char('s') => state.sort_by_value = !state.sort_by_value,
        KeyCode::Char('?') | KeyCode::Char('h') => state.show_help = !state.show_help,
        KeyCode::Up => move_selection(state, -1),
        KeyCode::Down => move_selection(state, 1),
        KeyCode::PageUp => move_selection(state, -10),
        KeyCode::PageDown => move_selection(state, 10),
        KeyCode::Home => state.selected = visible_sensors(state)
            .first()
            .map(|s| (s.chip.clone(), s.label.clone())),
        KeyCode::End => state.selected = visible_sensors(state)
            .last()
            .map(|s| (s.chip.clone(), s.label.clone())),
        _ => {}
    }
}

// ───────────────────────────── CLI (text mode, no TUI) ─────────────────────────────
fn print_list(sensors: &Sensors) {
    let version = Sensors::version();
    println!("libsensors {} — {} sensors", version, sensors.entries.len());
    for s in &sensors.entries {
        let st = status_of(s);
        println!(
            "{:<20} {:<12} {:<8} {:<10} min={} max={} crit={} {}",
            s.chip,
            s.label,
            s.kind.label(),
            fmt_value(s.kind, s.value),
            fmt_threshold(s.kind, s.min),
            fmt_threshold(s.kind, s.max),
            fmt_threshold(s.kind, s.crit),
            status_label(st),
        );
    }
}

// ───────────────────────────── Main loop ─────────────────────────────
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let list_mode = args.iter().any(|a| a == "--list" || a == "-l");

    let mut sensors = Sensors::new()?;
    if list_mode {
        print_list(&sensors);
        return Ok(());
    }

    let version = Sensors::version();

    let mut terminal = ratatui::init();

    let mut state = UiState {
        sensors: sensors.entries.clone(),
        version,
        filter: Filter::All,
        sort_by_value: true,
        show_help: false,
        selected: None,
        last_update: SystemTime::now(),
        force_refresh: false,
    };

    let mut running = true;
    let mut last_refresh = SystemTime::now();
    let refresh_interval = Duration::from_secs(1);

    while running {
        terminal.draw(|f| draw_ui(f, &state))?;

        if event::poll(Duration::from_millis(100))? {
            match event::read() {
                Ok(Event::Key(k)) => handle_key(k, &mut state, &mut running),
                Ok(_) => {}
                Err(_) => {}
            }
        }

        if state.force_refresh || last_refresh.elapsed().unwrap_or_default() >= refresh_interval {
            sensors.refresh()?;
            state.sensors = sensors.entries.clone();
            state.last_update = SystemTime::now();
            state.force_refresh = false;
            last_refresh = SystemTime::now();
        }
    }

    ratatui::restore();

    Ok(())
}
