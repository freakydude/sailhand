//! Records the flow of a print in a file of its own (the setting `record_flow`, off by default): a header with what
//! the print runs on, then one row for every status update that touches the recorded objects, and events (pause,
//! tool change, ...) as rows of their own. Nothing is evaluated here.

use crate::sources::klipper::PrintState;
use crate::sources::moonraker_server::files::FileMetadata;
use crate::sources::spoolman::Spool;
use crate::sources::{ObjectStore, PrinterStatus};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

/// Recorded prints kept in the directory; the oldest ones are removed when a new one starts.
const PRINTS_KEPT: usize = 100;
const FILE_PREFIX: &str = "flow-";
const FILE_SUFFIX: &str = ".csv";
const COLUMNS: &str = "t_ms,kind,live_extruder_velocity,live_velocity,flow_pct,flow_raw,flow_calibrated,\
encoder_flow,flowguard_level,bias_raw,temperature,target,power,speed_factor,extrude_factor,file_position,gate,tool,\
layer,event,detail";
/// Columns an event row fills: time, kind, event and detail.
const EVENT_COLUMNS_USED: usize = 4;
/// Objects whose updates make a row.
const RECORDED_OBJECTS: &[&str] =
    &["extruder", "motion_report", "mmu", "gcode_move", "virtual_sdcard", "print_stats", "toolchanger"];

/// Where and for which printer to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowRecording {
    /// The folder of the log, where the files of the prints go.
    pub directory: PathBuf,
    /// The name of the printer, part of the file name.
    pub printer: String,
}

/// What the header needs from outside the status: Moonraker's job and the file's metadata, the spools of the gates.
#[derive(Debug, Default)]
pub(super) struct HeaderFacts {
    pub job_id: Option<String>,
    pub metadata: Option<FileMetadata>,
    pub spools: HashMap<u32, Spool>,
    /// Moonraker's update manager knows the installed version of Happy Hare.
    pub happy_hare_version: Option<String>,
}

enum Phase {
    Idle,
    /// A print runs, the file waits for the header's facts; the rows are kept until then.
    Starting {
        buffer: Vec<String>,
        ended: bool,
        requested: bool,
    },
    Recording(BufWriter<File>),
    /// Writing failed; nothing more until the print is over.
    Failed,
}

/// The values the events follow.
#[derive(Debug, Default)]
struct Seen {
    state: PrintState,
    gate: String,
    tool: String,
    layer: String,
    target: String,
    speed_factor: String,
    extrude_factor: String,
    flowguard_trigger: String,
    /// Where Sailhand's flow value comes from; written when it changes, not in every row.
    flow_origin: String,
}

pub(super) struct FlowRecorder {
    config: Option<FlowRecording>,
    phase: Phase,
    seen: Option<Seen>,
    started_ms: u64,
    filename: String,
}

impl FlowRecorder {
    pub(super) fn new(config: Option<FlowRecording>) -> Self {
        Self { config, phase: Phase::Idle, seen: None, started_ms: 0, filename: String::new() }
    }

    pub(super) fn enabled(&self) -> bool {
        self.config.is_some()
    }

    /// Takes a status update: starts a recording with a print, writes the row and the events, ends it with the print.
    pub(super) fn observe(&mut self, store: &ObjectStore, status: &PrinterStatus, changes: &Value, now_ms: u64) {
        if self.config.is_none() {
            return;
        }
        let state = status.job.state;
        let active = matches!(state, PrintState::Printing | PrintState::Paused);
        if matches!(self.phase, Phase::Idle) && active {
            self.phase = Phase::Starting { buffer: Vec::new(), ended: false, requested: false };
            self.started_ms = now_ms;
            self.filename = status.job.filename.clone();
            self.seen = None;
        }
        if matches!(self.phase, Phase::Idle | Phase::Failed) {
            if !active {
                self.phase = Phase::Idle;
            }
            return;
        }
        let current = seen(store, status);
        let events = match &self.seen {
            Some(before) => events(before, &current),
            None if current.flow_origin.is_empty() => Vec::new(),
            None => vec![("flow_origin", current.flow_origin.clone())],
        };
        self.seen = Some(current);
        for (event, detail) in events {
            self.push(event_row(now_ms - self.started_ms, event, &detail));
        }
        let touched = changes
            .as_object()
            .is_some_and(|objects| objects.keys().any(|name| RECORDED_OBJECTS.contains(&name.as_str())));
        if touched {
            self.push(row(now_ms - self.started_ms, store, status));
        }
        if !active {
            self.push(event_row(now_ms - self.started_ms, "end", state_name(state)));
            self.finish();
        }
    }

    /// Whether the header's facts are wanted now; asked once per print.
    pub(super) fn wants_facts(&mut self) -> bool {
        match &mut self.phase {
            Phase::Starting { requested, ended: false, .. } if !*requested => {
                *requested = true;
                true
            }
            _ => false,
        }
    }

    /// The facts arrived: opens the file, writes the header and the rows kept so far.
    pub(super) fn begin(&mut self, store: &ObjectStore, facts: &HeaderFacts, unix_ms: u64) {
        let Phase::Starting { buffer, ended, .. } = std::mem::replace(&mut self.phase, Phase::Idle) else { return };
        let Some(config) = &self.config else { return };
        let job_id = facts.job_id.as_deref().unwrap_or("unknown");
        let name = file_name(unix_ms, &config.printer, job_id);
        let header = header(store, facts, &config.printer, job_id, &self.filename, unix_ms);
        match open(&config.directory, &name, &header, &buffer) {
            Ok(file) => {
                log::info!("recording the flow of this print in {name}");
                self.phase = Phase::Recording(file);
                if ended {
                    self.finish();
                }
            }
            Err(error) => {
                log::warn!("cannot record the flow in {name}: {error}");
                self.phase = if ended { Phase::Idle } else { Phase::Failed };
            }
        }
    }

    fn push(&mut self, line: String) {
        match &mut self.phase {
            Phase::Starting { buffer, .. } => buffer.push(line),
            Phase::Recording(file) => {
                // Flushed per line: a print that ends with the process leaves a complete record.
                if let Err(error) = writeln!(file, "{line}").and_then(|()| file.flush()) {
                    log::warn!("cannot record the flow: {error}");
                    self.phase = Phase::Failed;
                }
            }
            Phase::Idle | Phase::Failed => {}
        }
    }

    fn finish(&mut self) {
        match &mut self.phase {
            Phase::Starting { ended, .. } => *ended = true,
            Phase::Recording(file) => {
                if let Err(error) = file.flush() {
                    log::warn!("cannot record the flow: {error}");
                }
                self.phase = Phase::Idle;
            }
            Phase::Idle | Phase::Failed => self.phase = Phase::Idle,
        }
        self.seen = None;
    }
}

fn open(directory: &Path, name: &str, header: &str, rows: &[String]) -> io::Result<BufWriter<File>> {
    fs::create_dir_all(directory)?;
    let mut file = BufWriter::new(File::create(directory.join(name))?);
    writeln!(file, "{header}{COLUMNS}")?;
    for row in rows {
        writeln!(file, "{row}")?;
    }
    file.flush()?;
    remove_old(directory, name);
    Ok(file)
}

/// Keeps the newest [`PRINTS_KEPT`] files; the names start with the date.
fn remove_old(directory: &Path, current: &str) {
    let Ok(entries) = fs::read_dir(directory) else { return };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(FILE_PREFIX) && name.ends_with(FILE_SUFFIX))
        .collect();
    names.sort();
    let surplus = names.len().saturating_sub(PRINTS_KEPT);
    for name in names.into_iter().take(surplus).filter(|name| name != current) {
        if let Err(error) = fs::remove_file(directory.join(&name)) {
            log::warn!("cannot remove the old record {name}: {error}");
        }
    }
}

fn file_name(unix_ms: u64, printer: &str, job_id: &str) -> String {
    let printer: String =
        printer.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    let job: String = job_id.chars().filter(char::is_ascii_alphanumeric).collect();
    format!("{FILE_PREFIX}{}-{printer}-{job}{FILE_SUFFIX}", utc_stamp(unix_ms))
}

/// `YYYYmmdd-HHMMSS` in UTC.
fn utc_stamp(unix_ms: u64) -> String {
    let seconds = unix_ms / 1000;
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = if month_part < 10 { month_part + 3 } else { month_part - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}", rest / 3600, rest % 3600 / 60, rest % 60)
}

fn header(
    store: &ObjectStore,
    facts: &HeaderFacts,
    printer: &str,
    job_id: &str,
    filename: &str,
    unix_ms: u64,
) -> String {
    let mut text = String::new();
    let mut line = |name: &str, value: &str| {
        let _ = writeln!(text, "# {name}: {value}");
    };
    line("format", "sailhand flow record 1");
    line("start_utc", &utc_stamp(unix_ms));
    line("job_id", job_id);
    line("printer", printer);
    line("file", filename);
    if let Some(metadata) = &facts.metadata {
        line("slicer", metadata.slicer.as_deref().unwrap_or_default());
        line("slicer_material", &metadata.filament_types.join(";"));
        line("slicer_filament", &metadata.filament_names.join(";"));
        line("slicer_nozzle_diameter", &optional(metadata.nozzle_diameter));
        line("slicer_layer_height", &optional(metadata.layer_height));
        line("slicer_first_layer_temperature", &optional(metadata.first_layer_extr_temp));
        line("slicer_temperatures", &metadata.filament_temps.iter().map(f64::to_string).collect::<Vec<_>>().join(";"));
    }
    let settings = store.object("configfile").and_then(|configfile| configfile.get("settings"));
    let extruder = settings.and_then(|settings| settings.get("extruder"));
    for key in ["filament_diameter", "nozzle_diameter", "rotation_distance", "pressure_advance"] {
        line(&format!("extruder_{key}"), &extruder.and_then(|section| section.get(key)).map(plain).unwrap_or_default());
    }
    if let Some(mmu) = store.object("mmu") {
        line("happy_hare_version", facts.happy_hare_version.as_deref().unwrap_or_default());
        line("mmu_sensor", sensor_kind(mmu));
        gate_lines(mmu, facts, &mut line);
    }
    text
}

/// What reports the flow: the encoder, a proportional sensor (PSF) or only switches.
fn sensor_kind(mmu: &Value) -> &'static str {
    if mmu.get("encoder").is_some_and(Value::is_object) {
        "encoder"
    } else if mmu.get("sensors").is_some_and(|sensors| sensors.get("filament_proportional").is_some()) {
        "proportional (PSF)"
    } else {
        "switch"
    }
}

fn gate_lines(mmu: &Value, facts: &HeaderFacts, line: &mut impl FnMut(&str, &str)) {
    let list = |name: &str| mmu.get(name).and_then(Value::as_array).cloned().unwrap_or_default();
    let (materials, colors, spool_ids) = (list("gate_material"), list("gate_color"), list("gate_spool_id"));
    for gate in 0..materials.len().max(colors.len()).max(spool_ids.len()) {
        let at = |values: &[Value]| values.get(gate).map(plain).unwrap_or_default();
        let spool_id = spool_ids.get(gate).and_then(Value::as_u64).and_then(|id| u32::try_from(id).ok());
        let filament = spool_id.and_then(|id| facts.spools.get(&id)).map_or_else(String::new, |spool| {
            format!("{} {} {}", spool.vendor, spool.filament_name, spool.material).trim().to_owned()
        });
        line(
            &format!("gate_{gate}"),
            &format!(
                "material={} color={} spool_id={} spoolman_filament={filament}",
                at(&materials),
                at(&colors),
                at(&spool_ids)
            ),
        );
    }
}

fn optional(value: Option<f64>) -> String {
    value.map(|number| number.to_string()).unwrap_or_default()
}

/// A JSON value without quotes; `null` is empty.
fn plain(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn field(store: &ObjectStore, object: &str, path: &[&str]) -> String {
    let mut value = store.object(object);
    for key in path {
        value = value.and_then(|value| value.get(key));
    }
    value.map(plain).unwrap_or_default()
}

fn seen(store: &ObjectStore, status: &PrinterStatus) -> Seen {
    Seen {
        state: status.job.state,
        flow_origin: flow_origin(status),
        gate: field(store, "mmu", &["gate"]),
        tool: seen_tool(store),
        layer: field(store, "print_stats", &["info", "current_layer"]),
        target: field(store, "extruder", &["target"]),
        speed_factor: field(store, "gcode_move", &["speed_factor"]),
        extrude_factor: field(store, "gcode_move", &["extrude_factor"]),
        flowguard_trigger: field(store, "mmu", &["flowguard", "trigger"]),
    }
}

fn events(before: &Seen, now: &Seen) -> Vec<(&'static str, String)> {
    let mut events = Vec::new();
    match (before.state, now.state) {
        (PrintState::Printing, PrintState::Paused) => events.push(("pause", String::new())),
        (PrintState::Paused, PrintState::Printing) => events.push(("resume", String::new())),
        _ => {}
    }
    let mut changed = |event, before: &str, now: &str| {
        if before != now {
            events.push((event, format!("{before}>{now}")));
        }
    };
    changed("gate_change", &before.gate, &now.gate);
    changed("tool_change", &before.tool, &now.tool);
    changed("layer_change", &before.layer, &now.layer);
    changed("target_change", &before.target, &now.target);
    changed("speed_factor_change", &before.speed_factor, &now.speed_factor);
    changed("extrude_factor_change", &before.extrude_factor, &now.extrude_factor);
    changed("flow_origin", &before.flow_origin, &now.flow_origin);
    if before.flowguard_trigger != now.flowguard_trigger && !now.flowguard_trigger.is_empty() {
        events.push(("flowguard_trigger", now.flowguard_trigger.clone()));
    }
    events
}

fn flow_origin(status: &PrinterStatus) -> String {
    let flow = status.mmu.as_ref().and_then(|mmu| mmu.flow_rate);
    flow.map(|flow| format!("{} {}", if flow.is_measured() { "measured" } else { "estimated" }, flow.origin))
        .unwrap_or_default()
}

fn state_name(state: PrintState) -> &'static str {
    match state {
        PrintState::Standby => "standby",
        PrintState::Printing => "printing",
        PrintState::Paused => "paused",
        PrintState::Complete => "complete",
        PrintState::Cancelled => "cancelled",
        PrintState::Error => "error",
    }
}

/// A row of the event kind; the other columns stay empty.
fn event_row(t_ms: u64, event: &str, detail: &str) -> String {
    let empty_cells = COLUMNS.split(',').count() - EVENT_COLUMNS_USED;
    format!("{t_ms},event{},{event},{}", ",".repeat(empty_cells), csv_text(detail))
}

fn csv_text(text: &str) -> String {
    if text.contains([',', '"', '\n']) { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.to_owned() }
}

fn row(t_ms: u64, store: &ObjectStore, status: &PrinterStatus) -> String {
    let flow = status.mmu.as_ref().and_then(|mmu| mmu.flow_rate);
    let cells = [
        t_ms.to_string(),
        "row".to_owned(),
        field(store, "motion_report", &["live_extruder_velocity"]),
        field(store, "motion_report", &["live_velocity"]),
        flow.map(|flow| flow.value.to_string()).unwrap_or_default(),
        field(store, "mmu", &["sync_feedback_flow_rate"]),
        field(store, "mmu", &["sync_feedback_flow_rate_calibrated"]),
        field(store, "mmu", &["encoder", "flow_rate"]),
        field(store, "mmu", &["flowguard", "level"]),
        field(store, "mmu", &["sync_feedback_bias_raw"]),
        field(store, "extruder", &["temperature"]),
        field(store, "extruder", &["target"]),
        field(store, "extruder", &["power"]),
        field(store, "gcode_move", &["speed_factor"]),
        field(store, "gcode_move", &["extrude_factor"]),
        field(store, "virtual_sdcard", &["file_position"]),
        field(store, "mmu", &["gate"]),
        seen_tool(store),
        field(store, "print_stats", &["info", "current_layer"]),
        String::new(),
        String::new(),
    ];
    cells.iter().map(|cell| csv_text(cell)).collect::<Vec<_>>().join(",")
}

fn seen_tool(store: &ObjectStore) -> String {
    match field(store, "mmu", &["tool"]) {
        tool if !tool.is_empty() => tool,
        _ => field(store, "toolchanger", &["tool"]),
    }
}

#[cfg(test)]
mod tests;
