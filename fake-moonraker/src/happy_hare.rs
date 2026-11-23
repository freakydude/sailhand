//! Simplified Happy Hare simulation (objects `mmu` and `mmu_machine`).
//!
//! Field names follow the Happy Hare v4 reference "Printer Variables". Operations run without
//! elapsed time: `action` is always `Idle`, loading and unloading complete immediately.

use crate::gcode::{CommandError, MmuCommand, ServoPosition};
use crate::machine::{BufferKind, MmuUnitSpec, SpoolmanMode};
use crate::spoolman;
use serde_json::{Map, Value, json};

const VERSION: &str = "4.1.0";
const NONE_SELECTED: i32 = -1;
/// `gate` and `tool` while the bypass is selected.
const BYPASS: i32 = -2;
const GATE_UNKNOWN: i32 = -1;
const GATE_EMPTY: i32 = 0;
const GATE_AVAILABLE: i32 = 1;
const FILAMENT_POSITION_UNLOADED: i32 = 0;
const FILAMENT_POSITION_LOADED: i32 = 10;
/// Where Happy Hare's `filament_pos` passes the sensors of the way: gate exit, extruder entry, toolhead.
const FILAMENT_POSITION_GATE_SENSOR: i32 = 1;
const FILAMENT_POSITION_EXTRUDER_SENSOR: i32 = 5;
const FILAMENT_POSITION_TOOLHEAD_SENSOR: i32 = 8;
/// Millimetres from the gate to the nozzle, for `filament_position`.
const PATH_LENGTH: f64 = 1712.4;
const UNIT_NAME: &str = "unit0";
/// `mmu.selector.servo` (`mmu_linear_servo_selector.py`).
const SERVO_UP: &str = "Up";
const SERVO_DOWN: &str = "Down";
const SERVO_MOVE: &str = "Move";

pub(crate) struct Spool {
    pub(crate) name: &'static str,
    pub(crate) material: &'static str,
    pub(crate) vendor: &'static str,
    pub(crate) color: &'static str,
    pub(crate) rgb: [f64; 3],
    pub(crate) temperature: i32,
}

/// The demo filaments the gates cycle through; the two after them belong to the multi-colour spools of Spoolman.
pub(crate) const BASIC_FILAMENTS: usize = 4;

pub(crate) const SPOOLS: [Spool; 6] = [
    Spool { name: "Red PLA", material: "PLA", vendor: "Fake", color: "red", rgb: [1.0, 0.0, 0.0], temperature: 210 },
    Spool {
        name: "Green PETG",
        material: "PETG",
        vendor: "Fake",
        color: "green",
        rgb: [0.0, 0.8, 0.0],
        temperature: 235,
    },
    Spool { name: "Blue PLA", material: "PLA", vendor: "Fake", color: "blue", rgb: [0.0, 0.2, 1.0], temperature: 210 },
    Spool {
        name: "Black ABS",
        material: "ABS",
        vendor: "Fake",
        color: "black",
        rgb: [0.1, 0.1, 0.1],
        temperature: 250,
    },
    // The first colour of the multi-colour filaments, which is what Happy Hare reports for a gate.
    Spool {
        name: "Gradient Red+Green",
        material: "PLA",
        vendor: "Fake",
        color: "red",
        rgb: [0.89, 0.12, 0.17],
        temperature: 215,
    },
    Spool {
        name: "Silk Red+Gold+Black",
        material: "PLA",
        vendor: "Fake",
        color: "red",
        rgb: [1.0, 0.0, 0.02],
        temperature: 220,
    },
];

/// Spools of the gates, in the order the gates take them: the first is one colour, then two, then three colours.
const GATE_SPOOLS: [u32; 7] = [1, 7, 8, 4, 5, 6, 2];

/// Tension from which tangle prevention raises the gear current, and below which it lets go (Happy Hare defaults).
const TANGLE_THRESHOLD: f64 = 0.3;
const TANGLE_RELEASE: f64 = 0.2;

/// What the proportional buffer reports for one situation.
struct Buffer {
    state: &'static str,
    bias: f64,
    flow_rate: f64,
    flowguard_level: f64,
}

#[derive(Debug, Clone)]
pub struct HappyHare {
    gates: usize,
    homed: bool,
    gate: i32,
    tool: i32,
    loaded: bool,
    /// Last movement of the filament: 1 load, -1 unload, 0 none (`filament_direction`).
    direction: i32,
    /// Held by `FAKE_MMU_FILAMENT`; otherwise the position follows `loaded`.
    held_position: Option<i32>,
    toolchanges: u32,
    tool_to_gate: Vec<usize>,
    /// EndlessSpool (`endless_spool_enabled`, off as in Happy Hare's default) and the group of each gate.
    endless_spool: bool,
    endless_spool_groups: Vec<u32>,
    gate_status: Vec<i32>,
    /// Spoolman spool of each gate.
    gate_spool: Vec<Option<u32>>,
    /// The filament of each gate as Happy Hare keeps it: a copy of what the spool had when Happy Hare took it.
    gate_filament: Vec<GateFilament>,
    /// `spoolman_support`: `push` with Moonraker's Spoolman, `off` without.
    spoolman: SpoolmanMode,
    /// What Happy Hare told Moonraker's active spool since the script began (`spoolman_set_active_spool`): the spool
    /// of the gate after a load, `None` at the start of an unload. The session hands them on and empties the list.
    active_spool_events: Vec<Option<u32>>,
    /// Console lines of the script (`log_error` is `!! ...`, `log_always` is `// ...`); the session sends them.
    console: Vec<String>,
    /// The filament pulls hard on the buffer: tension at the stop and tangle prevention raises the gear current.
    boosting: bool,
    /// `mmu.selector.servo` of a linear selector with a servo, as on an ERCF.
    servo: &'static str,
    /// The sync-feedback buffer; the variant shows in `mmu.sensors` and in what the buffer fields hold.
    buffer: BufferKind,
    /// An encoder: `mmu.encoder`, `mmu.sensors.encoder` and the encoder mode of FlowGuard.
    encoder: bool,
    /// The units of a v4 machine with several; empty for the older profiles, whose one unit is `unit0`.
    units: Vec<MmuUnitSpec>,
    /// `mmu.unit`: the unit of the selected gate, which stays when no gate is selected (`mmu_controller.py` L325-327,
    /// L3865-3866). Only shown for `units`.
    unit_selected: usize,
}

/// What a vendor's unit looks like by default (`MmuUnitProfile` in `mmu_unit.py` L96-125 at 7df830f): the ERCF has a
/// selectable bypass and variable rotation distances, the Tradrack neither; both have a linear selector with a servo
/// and a filament buffer.
struct VendorProfile {
    has_bypass: bool,
    variable_rotation_distances: bool,
    /// The unit has a `mmu_shared_exit` sensor: `installer/mmu_types/Kconfig.tradrack` L9 selects
    /// `MMU_HAS_SENSOR_SHARED_EXIT` for the Tradrack. `Kconfig.ercf` does not (only the optional entry sensors, L32
    /// and L43).
    shared_exit_sensor: bool,
}

fn vendor_profile(vendor: &str) -> VendorProfile {
    match vendor {
        "ERCF" => VendorProfile { has_bypass: true, variable_rotation_distances: true, shared_exit_sensor: false },
        "Tradrack" => VendorProfile { has_bypass: false, variable_rotation_distances: false, shared_exit_sensor: true },
        _ => VendorProfile { has_bypass: false, variable_rotation_distances: false, shared_exit_sensor: false },
    }
}

/// The sensor names (`mmu_constants.py` L160-178) and the prefix of the Klipper object of a Happy Hare switch sensor
/// (`mmu_sensor_utils.py` L395).
const SENSOR_PREFIX: &str = "filament_switch_sensor ";
const SENSOR_ENTRY: &str = "mmu_entry";
const SENSOR_EXIT: &str = "mmu_exit";
const SENSOR_SHARED_EXIT: &str = "mmu_shared_exit";
const SENSOR_EXTRUDER: &str = "extruder";
const SENSOR_TOOLHEAD: &str = "toolhead";
const SENSOR_COMPRESSION: &str = "filament_compression";
const SENSOR_TENSION: &str = "filament_tension";
const SENSOR_PROPORTIONAL: &str = "filament_proportional";
const SENSOR_ENCODER: &str = "encoder";

/// What `filament_sensors` needs of a unit. Profile choices: every gate has an entry and an exit sensor (as on the
/// recorded single-lane machine; `Kconfig.ercf` and `Kconfig.tradrack` make both optional), and every unit has a
/// buffer with compression and tension sensors but no proportional one.
struct SensorUnit {
    first_gate: usize,
    gates: usize,
    /// The unit name if the unit has a shared exit sensor: the owner of `<unit>:mmu_shared_exit`.
    shared_exit: Option<&'static str>,
    toolhead: &'static str,
    buffer: Option<&'static str>,
}

/// `mmu_extruder_wrapper.py` L36-39: what a fresh extruder wrapper reports (`UNKNOWN_FILAMENT_COLOR` of
/// `mmu_constants.py` L341).
const UNKNOWN_FILAMENT_COLOR: &str = "#808182E3";

/// `default_extruder_temp` of `[mmu_parameters]` (`mmu_machine_parameters.py` L55); the temperature of a gate taken
/// from a spool is never lower (`components/mmu_server.py`, as the caller of this fake reported).
const DEFAULT_EXTRUDER_TEMP: i32 = 200;

/// Moonraker's `hostname` as `server.info` of the fake reports it (`printer.info`); the messages of `MMU_SPOOLMAN`
/// name it (`mmu_server.py` `_log_n_send` with `self.printer_hostname`).
const PRINTER_HOSTNAME: &str = "fake-printer";

/// The filament of the second gate with a spool differs from its spool at the start: someone edited the spool in
/// Spoolman after Happy Hare took the values; it only takes them again on assignment, REFRESH or restart
/// (`mmu_server.py`).
const EDITED_MATERIAL: &str = "PETG";
const EDITED_COLOR: &str = "ff8000";

/// `log_error` (`mmu_logger.py` L141-144) and `log_always` (L149-152) as the console shows them.
const ERROR_PREFIX: &str = "!! ";
const INFO_PREFIX: &str = "// ";

/// What Happy Hare keeps of the filament of a gate (its gate map). With Spoolman it is a copy of the values of the
/// spool, taken when the spool is assigned, on `MMU_SPOOLMAN REFRESH=1` or `SYNC=1` and at the start
/// (`mmu_server.py` `_get_filament_attr`, `mmu_controller.py` `_spoolman_sync`).
#[derive(Debug, Clone)]
struct GateFilament {
    name: String,
    material: String,
    vendor: String,
    /// `rrggbb`, a colour name of the demo filaments, or empty (a spool with several colours has no `color_hex`).
    color: String,
    rgb: [f64; 3],
    temperature: i32,
}

impl GateFilament {
    /// A demo filament, for a gate without Spoolman or without a spool.
    fn demo(gate: usize) -> Self {
        let spool = &SPOOLS[gate % BASIC_FILAMENTS];
        Self {
            name: spool.name.to_owned(),
            material: spool.material.to_owned(),
            vendor: spool.vendor.to_owned(),
            color: spool.color.to_owned(),
            rgb: spool.rgb,
            temperature: spool.temperature,
        }
    }

    /// What Happy Hare takes of a spool (`mmu_server.py` `_get_filament_attr`): name, material, vendor, the colour only
    /// from `color_hex` and the temperature `settings_extruder_temp` at least `default_extruder_temp`. `None` for a
    /// spool Spoolman does not know.
    fn from_spoolman(id: u32) -> Option<Self> {
        let spool = spoolman::initial_spool(id)?;
        let filament = &spool["filament"];
        let text = |field: &str| filament[field].as_str().unwrap_or_default().to_owned();
        let color = text("color_hex").trim_start_matches('#').to_ascii_lowercase();
        let temperature = filament["settings_extruder_temp"].as_i64().unwrap_or_default() as i32;
        Some(Self {
            name: text("name"),
            material: text("material"),
            vendor: filament["vendor"]["name"].as_str().unwrap_or_default().to_owned(),
            rgb: hex_rgb(&color).unwrap_or_default(),
            color,
            temperature: temperature.max(DEFAULT_EXTRUDER_TEMP),
        })
    }
}

/// What `MMU_GATE_MAP` sets on a gate; `None` leaves the value.
#[derive(Debug, Clone, Default)]
struct GateChange {
    name: Option<String>,
    material: Option<String>,
    vendor: Option<String>,
    color: Option<String>,
    temperature: Option<i32>,
}

/// `rrggbb` as Happy Hare's `gate_color_rgb` (three floats from 0 to 1); colour names are not simulated.
fn hex_rgb(color: &str) -> Option<[f64; 3]> {
    if color.len() != 6 {
        return None;
    }
    let channel =
        |index: usize| u8::from_str_radix(color.get(index..index + 2)?, 16).ok().map(|part| f64::from(part) / 255.0);
    Some([channel(0)?, channel(2)?, channel(4)?])
}

impl HappyHare {
    /// Without `spoolman` the gates have no spools; Happy Hare keeps their filament in its own gate map.
    pub fn new(gates: usize, spoolman: SpoolmanMode) -> Self {
        Self::build(gates, Vec::new(), spoolman)
    }

    /// A v4 machine of several units; their gates follow each other (`mmu_machine.py` L110-123).
    pub fn with_units(units: &[MmuUnitSpec], spoolman: SpoolmanMode) -> Self {
        Self::build(units.iter().map(|unit| unit.gates).sum(), units.to_vec(), spoolman)
    }

    fn build(gates: usize, units: Vec<MmuUnitSpec>, spoolman: SpoolmanMode) -> Self {
        // The last gate stays empty so the UI also has to show an empty slot.
        let mut gate_status = vec![GATE_AVAILABLE; gates];
        if gates > 2 {
            gate_status[gates - 1] = GATE_EMPTY;
        }
        let mut spools = GATE_SPOOLS.into_iter().filter(|_| spoolman != SpoolmanMode::Off);
        let gate_spool: Vec<Option<u32>> =
            gate_status.iter().map(|status| if *status == GATE_AVAILABLE { spools.next() } else { None }).collect();
        let mut gate_filament: Vec<GateFilament> = (0..gates)
            .map(|gate| {
                gate_spool[gate].and_then(GateFilament::from_spoolman).unwrap_or_else(|| GateFilament::demo(gate))
            })
            .collect();
        if let Some(gate) =
            gate_spool.iter().enumerate().filter(|(_, spool)| spool.is_some()).nth(1).map(|(gate, _)| gate)
        {
            let edited = &mut gate_filament[gate];
            edited.material = EDITED_MATERIAL.to_owned();
            edited.color = EDITED_COLOR.to_owned();
            edited.rgb = hex_rgb(EDITED_COLOR).unwrap_or_default();
        }
        Self {
            gates,
            homed: false,
            gate: NONE_SELECTED,
            tool: NONE_SELECTED,
            loaded: false,
            direction: 0,
            held_position: None,
            toolchanges: 0,
            tool_to_gate: (0..gates).collect(),
            endless_spool: false,
            endless_spool_groups: (0..gates as u32).collect(),
            gate_status,
            gate_spool,
            gate_filament,
            spoolman,
            active_spool_events: Vec::new(),
            console: Vec::new(),
            boosting: false,
            servo: SERVO_UP,
            buffer: BufferKind::Proportional,
            encoder: false,
            units,
            unit_selected: 0,
        }
    }

    /// Replaces the proportional buffer without encoder of the default machine.
    pub fn with_feedback(mut self, buffer: BufferKind, encoder: bool) -> Self {
        self.buffer = buffer;
        self.encoder = encoder;
        self
    }

    /// The units to describe: the profile's, or the one `unit0` of the older profiles.
    fn has_units(&self) -> bool {
        !self.units.is_empty()
    }

    /// The first gate of each unit.
    fn first_gates(&self) -> Vec<usize> {
        let mut first = 0;
        self.units
            .iter()
            .map(|unit| {
                let start = first;
                first += unit.gates;
                start
            })
            .collect()
    }

    fn unit_of_gate(&self, gate: usize) -> usize {
        let first_gates = self.first_gates();
        first_gates.iter().rposition(|first| *first <= gate).unwrap_or(0)
    }

    /// The unit that shows the bypass (`mmu_unit.py` L139: `show_bypass`; only one may).
    fn bypass_unit(&self) -> Option<usize> {
        self.units.iter().position(|unit| vendor_profile(unit.vendor).has_bypass)
    }

    /// The distinct extruders of the units in the order of the units (`mmu_unit.py` L325-332: one object each).
    fn extruder_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = Vec::new();
        for unit in &self.units {
            if !names.contains(&unit.extruder) {
                names.push(unit.extruder);
            }
        }
        names
    }

    /// The distinct `[mmu_toolhead]` sections of the units, in the order of the units.
    fn toolhead_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = Vec::new();
        for unit in &self.units {
            if !names.contains(&unit.toolhead) {
                names.push(unit.toolhead);
            }
        }
        names
    }

    /// The distinct `[mmu_buffer]` sections of the units, in the order of the units.
    fn buffer_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = Vec::new();
        for buffer in self.units.iter().filter_map(|unit| unit.buffer) {
            if !names.contains(&buffer) {
                names.push(buffer);
            }
        }
        names
    }

    /// `print_state` as in `print_stats`; some commands refuse while printing.
    pub fn apply(&mut self, command: MmuCommand, print_state: &str) -> Result<(), CommandError> {
        match command {
            MmuCommand::Home => {
                self.homed = true;
                Ok(())
            }
            MmuCommand::Select { gate } => self.select_gate(gate),
            MmuCommand::ChangeTool { tool } => self.change_tool(tool),
            MmuCommand::Load => self.load(),
            MmuCommand::Unload => self.unload(),
            MmuCommand::Eject { gate, force } => self.eject(gate, force),
            MmuCommand::MotorsOff => {
                self.homed = false;
                Ok(())
            }
            MmuCommand::CheckGate { gate, all } => self.check_gates(gate, all),
            MmuCommand::Preload { gate } => self.preload(gate, print_state == "printing"),
            MmuCommand::Servo { position } => {
                self.move_servo(position);
                Ok(())
            }
            MmuCommand::GateMap { gate, spool_id, name, material, vendor, color, temperature, weights_given } => {
                self.gate_map(gate, spool_id, GateChange { name, material, vendor, color, temperature }, weights_given)
            }
            MmuCommand::Spoolman { gate, spool_id, refresh, sync } => {
                self.spoolman_command(gate, spool_id, refresh || sync)
            }
            MmuCommand::SelectBypass => self.select_bypass(),
            MmuCommand::MapTool { tool, gate } => self.map_tool(tool, gate),
            MmuCommand::ResetToolMap => {
                self.tool_to_gate = (0..self.gates).collect();
                self.match_tool_to_gate();
                Ok(())
            }
            MmuCommand::EndlessSpool { enabled, groups } => self.set_endless_spool(enabled, groups),
            MmuCommand::FakeFilament { position, direction } => {
                self.held_position = Some(position);
                self.direction = direction;
                Ok(())
            }
            MmuCommand::FakeTangle { boosting } => {
                self.boosting = boosting;
                Ok(())
            }
        }
    }

    pub fn status(&self, print_state: &str) -> Map<String, Value> {
        let mut objects = Map::new();
        objects.insert("mmu".to_owned(), self.mmu_json(print_state));
        objects.insert("mmu_machine".to_owned(), self.machine_json());
        if self.has_units() {
            for extruder in self.extruder_names() {
                objects.insert(format!("mmu_extruder {extruder}"), extruder_wrapper_json(extruder));
            }
            for unit in &self.units {
                objects.insert(format!("mmu_stepper {}_gear", unit.name), json!({}));
            }
            // `mmu_unit.py` L469 and L540 register the toolhead and buffer sections as objects; their own status
            // (`mmu_toolhead_wrapper.py` L275, `mmu_buffer.py` L353) is not simulated.
            for toolhead in self.toolhead_names() {
                objects.insert(format!("mmu_toolhead {toolhead}"), json!({}));
            }
            for buffer in self.buffer_names() {
                objects.insert(format!("mmu_buffer {buffer}"), json!({}));
            }
            return objects;
        }
        objects.insert("mmu_extruder extruder".to_owned(), json!({}));
        objects.insert(format!("mmu_stepper {UNIT_NAME}_gear"), json!({}));
        objects
    }

    /// The settings of a v4 machine with units: `[mmu_machine] units` (Klipper reports a list option as a list, in
    /// the order of the unit indexes; `config/base/mmu.cfg` L51) and one `[mmu_unit <name>]` each
    /// (`mmu_hardware.cfg` L137-155, `mmu_unit.py` L323: `extruder_name` defaults to `extruder`).
    fn unit_config_sections(&self) -> Vec<(String, Value)> {
        let names: Vec<&str> = self.units.iter().map(|unit| unit.name).collect();
        let mut sections =
            vec![("mmu_machine".to_owned(), json!({ "units": names })), ("mmu".to_owned(), json!({ "enabled": true }))];
        for unit in &self.units {
            let mut options = json!({
                "extruder_name": unit.extruder,
                "toolhead": unit.toolhead,
                "num_gates": unit.gates,
                "vendor": unit.vendor,
                "display_name": unit.display_name,
            });
            if let (Some(buffer), Some(options)) = (unit.buffer, options.as_object_mut()) {
                options.insert("buffer".to_owned(), json!(buffer));
            }
            sections.push((format!("mmu_unit {}", unit.name), options));
            sections.push((format!("mmu_stepper {}_gear", unit.name), json!({})));
        }
        // The sections the units name: `mmu_unit.py` L463 (toolhead) and L534 (buffer).
        for toolhead in self.toolhead_names() {
            sections.push((format!("mmu_toolhead {toolhead}"), json!({})));
        }
        for buffer in self.buffer_names() {
            sections.push((format!("mmu_buffer {buffer}"), self.buffer_options()));
        }
        sections
    }

    /// Klipper lists the defaults too; `analog_sensor_threshold` exists only for a proportional sensor
    /// (`unit/mmu_buffer.py` L200, default 0.9).
    fn buffer_options(&self) -> Value {
        match self.buffer {
            BufferKind::Proportional => json!({ "analog_sensor_threshold": 0.9 }),
            _ => json!({}),
        }
    }

    pub fn config_sections(&self) -> Vec<(String, Value)> {
        if self.has_units() {
            return self.unit_config_sections();
        }
        vec![
            ("mmu_machine".to_owned(), json!({ "num_units": 1 })),
            ("mmu".to_owned(), json!({ "enabled": true })),
            ("mmu_extruder extruder".to_owned(), json!({})),
            (format!("mmu_stepper {UNIT_NAME}_gear"), json!({})),
            // The section name is the user's choice (made up here); the default is listed as Klipper does.
            ("mmu_buffer buffer".to_owned(), self.buffer_options()),
        ]
    }

    fn select_gate(&mut self, gate: usize) -> Result<(), CommandError> {
        self.require_homed()?;
        self.check_gate(gate)?;
        if self.loaded {
            return Err(CommandError::new("Cannot change gate while filament is loaded"));
        }
        self.select(gate);
        Ok(())
    }

    /// Like `MMU_SELECT_BYPASS` (`mmu_select_bypass.py`): allowed on every MMU, refused while filament is loaded.
    fn select_bypass(&mut self) -> Result<(), CommandError> {
        if self.loaded {
            return Err(CommandError::new("Operation not possible. Filament is loaded"));
        }
        self.gate = BYPASS;
        self.tool = BYPASS;
        if let Some(unit) = self.bypass_unit() {
            self.unit_selected = unit;
        }
        Ok(())
    }

    /// Like `MMU_TTG_MAP TOOL= GATE=` (`mmu_ttg_map.py`): an empty gate becomes unknown, as it was probably just filled.
    fn map_tool(&mut self, tool: usize, gate: usize) -> Result<(), CommandError> {
        self.check_gate(gate)?;
        let mapped =
            self.tool_to_gate.get_mut(tool).ok_or_else(|| CommandError::new(format!("Tool T{tool} does not exist")))?;
        *mapped = gate;
        if self.gate_status[gate] == GATE_EMPTY {
            self.gate_status[gate] = GATE_UNKNOWN;
        }
        self.match_tool_to_gate();
        Ok(())
    }

    /// Like `ensure_ttg_match` (`mmu_gate_maps.py`): the selected tool follows the map of the selected gate.
    fn match_tool_to_gate(&mut self) {
        if let Ok(gate) = usize::try_from(self.gate) {
            self.select(gate);
        }
    }

    /// Like `MMU_ENDLESS_SPOOL` (`mmu_endless_spool.py`): the groups need one value per gate.
    fn set_endless_spool(&mut self, enabled: Option<bool>, groups: Option<Vec<u32>>) -> Result<(), CommandError> {
        if let Some(groups) = groups {
            if groups.len() != self.gates {
                return Err(CommandError::new(format!(
                    "Invalid GROUPS value. Expected {} comma-separated non-negative integers.",
                    self.gates
                )));
            }
            self.endless_spool_groups = groups;
        }
        if let Some(enabled) = enabled {
            self.endless_spool = enabled;
        }
        Ok(())
    }

    fn change_tool(&mut self, tool: usize) -> Result<(), CommandError> {
        self.require_homed()?;
        let gate =
            *self.tool_to_gate.get(tool).ok_or_else(|| CommandError::new(format!("Tool T{tool} does not exist")))?;
        self.require_filament_at(gate)?;
        // Another gate is unloaded first; the same gate stays loaded.
        if self.loaded && self.gate != gate as i32 {
            self.set_loaded(false);
        }
        self.select(gate);
        self.set_loaded(true);
        self.tool = tool as i32;
        self.toolchanges += 1;
        Ok(())
    }

    fn load(&mut self) -> Result<(), CommandError> {
        self.require_homed()?;
        if self.loaded {
            return Err(CommandError::new("Filament is already loaded"));
        }
        // Filament in the bypass was fed by hand; the fake takes it as there.
        if self.gate != BYPASS {
            let selected = usize::try_from(self.gate).map_err(|_| CommandError::new("No gate selected"))?;
            self.require_filament_at(selected)?;
        }
        self.set_loaded(true);
        Ok(())
    }

    /// Like `MMU_EJECT` (`mmu_eject.py`): while the filament of the selected gate is loaded, the command only unloads
    /// it (unless `FORCE`); otherwise it pulls the filament out of the gate, which is then empty.
    fn eject(&mut self, gate: Option<usize>, force: bool) -> Result<(), CommandError> {
        self.require_homed()?;
        let current = usize::try_from(self.gate).ok();
        let target = gate.or(current).ok_or_else(|| CommandError::new("No gate selected"))?;
        self.check_gate(target)?;
        if self.loaded && Some(target) == current && !force {
            return self.unload();
        }
        if self.loaded && Some(target) != current {
            return Err(CommandError::new("Cannot eject another gate while filament is loaded"));
        }
        self.set_loaded(false);
        self.gate_status[target] = GATE_EMPTY;
        Ok(())
    }

    /// Like `MMU_CHECK_GATE` (`mmu_check_gate.py`): a loaded gate counts as available without a check; for the others
    /// loaded filament is unloaded first and loaded again afterwards. The fake has no sensors, so a checked gate keeps
    /// what it had: empty stays empty, everything else becomes available.
    fn check_gates(&mut self, gate: Option<usize>, all: bool) -> Result<(), CommandError> {
        self.require_homed()?;
        let gates: Vec<usize> = match (gate, all) {
            (_, true) => (0..self.gates).collect(),
            (Some(gate), false) => vec![gate],
            (None, false) => {
                vec![usize::try_from(self.gate).map_err(|_| CommandError::new("Current gate is invalid"))?]
            }
        };
        for gate in gates {
            self.check_gate(gate)?;
            if self.gate_status[gate] != GATE_EMPTY {
                self.gate_status[gate] = GATE_AVAILABLE;
            }
        }
        Ok(())
    }

    /// Like `MMU_PRELOAD` (`mmu_preload.py`): parks filament pushed into the gate, so the gate has filament again.
    fn preload(&mut self, gate: Option<usize>, printing: bool) -> Result<(), CommandError> {
        if printing {
            return Err(CommandError::new("Operation not possible. Printer is actively printing"));
        }
        let current = usize::try_from(self.gate).ok();
        let target = gate.or(current).ok_or_else(|| CommandError::new("No gate selected"))?;
        self.check_gate(target)?;
        if self.loaded && Some(target) == current {
            return Err(CommandError::new("Operation not possible. Filament is loaded"));
        }
        self.gate_status[target] = GATE_AVAILABLE;
        Ok(())
    }

    /// `off` only cuts the power of the servo and keeps its position.
    fn move_servo(&mut self, position: Option<ServoPosition>) {
        self.servo = match position {
            Some(ServoPosition::Up) => SERVO_UP,
            Some(ServoPosition::Down) => SERVO_DOWN,
            Some(ServoPosition::Move) => SERVO_MOVE,
            Some(ServoPosition::Off) | None => self.servo,
        };
    }

    fn unload(&mut self) -> Result<(), CommandError> {
        self.require_homed()?;
        if !self.loaded {
            return Err(CommandError::new("Filament is not loaded"));
        }
        self.set_loaded(false);
        Ok(())
    }

    /// A load or an unload ends at its end of the way, moving in its direction.
    fn set_loaded(&mut self, loaded: bool) {
        if loaded != self.loaded {
            self.tell_moonraker_active_spool(loaded);
        }
        self.loaded = loaded;
        self.direction = if loaded { 1 } else { -1 };
        self.held_position = None;
    }

    fn filament_position(&self) -> i32 {
        self.held_position.unwrap_or(if self.loaded { FILAMENT_POSITION_LOADED } else { FILAMENT_POSITION_UNLOADED })
    }

    /// Happy Hare's `bowden_progress`: 0 to 100 while the filament moves through the bowden (positions 2 to 4), -1
    /// otherwise. A held position stands for a third of the way per step.
    fn bowden_progress(&self) -> f64 {
        match self.held_position {
            Some(position @ 2..=4) => f64::from(position - 1) * 100.0 / 3.0,
            _ => -1.0,
        }
    }

    fn select(&mut self, gate: usize) {
        self.gate = gate as i32;
        if self.has_units() {
            self.unit_selected = self.unit_of_gate(gate);
        }
        self.tool =
            self.tool_to_gate.iter().position(|mapped| *mapped == gate).map_or(NONE_SELECTED, |tool| tool as i32);
    }

    fn require_homed(&self) -> Result<(), CommandError> {
        if self.homed { Ok(()) } else { Err(CommandError::new("MMU is not homed")) }
    }

    fn check_gate(&self, gate: usize) -> Result<(), CommandError> {
        if gate < self.gates { Ok(()) } else { Err(CommandError::new(format!("Gate {gate} does not exist"))) }
    }

    fn require_filament_at(&self, gate: usize) -> Result<(), CommandError> {
        if self.gate_status[gate] == GATE_EMPTY {
            Err(CommandError::new(format!("Gate {gate} is empty")))
        } else {
            Ok(())
        }
    }

    /// Happy Hare tells Moonraker the active spool after a load (the spool of the gate, if it has one) and at the
    /// start of an unload (none), never with `spoolman_support: off` (`mmu_controller.py` `_spoolman_activate_spool`;
    /// `mmu_filament_movement.py` L2749 and L2897 at 5aa3d30).
    fn tell_moonraker_active_spool(&mut self, loading: bool) {
        if self.spoolman == SpoolmanMode::Off {
            return;
        }
        if !loading {
            self.active_spool_events.push(None);
        } else if let Some(spool) = self.selected_spool() {
            self.active_spool_events.push(Some(spool));
        }
    }

    /// The Spoolman spool of the selected gate.
    fn selected_spool(&self) -> Option<u32> {
        usize::try_from(self.gate).ok().and_then(|gate| self.gate_spool.get(gate).copied().flatten())
    }

    /// The active spool changes Happy Hare reported since the last call, oldest first.
    pub fn take_active_spool_events(&mut self) -> Vec<Option<u32>> {
        std::mem::take(&mut self.active_spool_events)
    }

    /// The console lines of the last commands, oldest first.
    pub fn take_console(&mut self) -> Vec<String> {
        std::mem::take(&mut self.console)
    }

    /// `MMU_GATE_MAP GATE= SPOOLID=`; a negative id takes the spool out. `commands/mmu_gate_map.py` `_run`:
    /// - `pull`: with one of the parameters Spoolman owns (material, vendor, colour, spool, name, weights) the command
    ///   only logs an error and does not fail; `TEMP` is ignored.
    /// - Otherwise the given values are set. A **changed** spool id makes Happy Hare take the filament of the spool
    ///   again (not with `off`), which overwrites the values given in the same call; the same id fetches nothing.
    ///   A spool sits on one gate only (`assign_spool_id`).
    fn gate_map(
        &mut self,
        gate: usize,
        spool_id: Option<i64>,
        change: GateChange,
        weights_given: bool,
    ) -> Result<(), CommandError> {
        self.check_gate(gate)?;
        if let Some(color) = &change.color {
            hex_rgb(color).ok_or_else(|| {
                CommandError::new("Color specification must be in form 'rrggbb' hexadecimal value (no '#')")
            })?;
        }
        if self.spoolman == SpoolmanMode::Pull {
            let spoolman_owns = change.material.is_some()
                || change.vendor.is_some()
                || change.color.is_some()
                || change.name.is_some()
                || spool_id.is_some()
                || weights_given;
            if spoolman_owns {
                self.console.push(format!(
                    "{ERROR_PREFIX}Spoolman mode is 'pull': Can only set gate status and speed override locally\nUse MMU_SPOOLMAN or update spoolman directly"
                ));
            }
            return Ok(());
        }
        self.set_local_values(gate, change);
        if let Some(spool_id) = spool_id {
            let spool = u32::try_from(spool_id).ok().filter(|id| *id > 0);
            if spool != self.gate_spool[gate] {
                self.assign_spool_id(gate, spool);
                if self.spoolman != SpoolmanMode::Off {
                    self.take_filament_of_spool(gate);
                }
            }
        }
        Ok(())
    }

    fn set_local_values(&mut self, gate: usize, change: GateChange) {
        let filament = &mut self.gate_filament[gate];
        if let Some(name) = change.name {
            filament.name = name;
        }
        if let Some(material) = change.material {
            filament.material = material;
        }
        if let Some(vendor) = change.vendor {
            filament.vendor = vendor;
        }
        if let Some(color) = change.color {
            filament.rgb = hex_rgb(&color).unwrap_or_default();
            filament.color = color;
        }
        if let Some(temperature) = change.temperature {
            filament.temperature = temperature;
        }
    }

    /// `mmu_gate_maps.py` `assign_spool_id`: the gate gets the spool, and no other gate keeps it.
    fn assign_spool_id(&mut self, gate: usize, spool: Option<u32>) {
        if spool.is_some() {
            for other in self.gate_spool.iter_mut().filter(|other| **other == spool) {
                *other = None;
            }
        }
        self.gate_spool[gate] = spool;
    }

    /// Copies the values of the gate's spool into the gate map; a spool Spoolman does not know changes nothing.
    fn take_filament_of_spool(&mut self, gate: usize) {
        if let Some(filament) = self.gate_spool[gate].and_then(GateFilament::from_spoolman) {
            self.gate_filament[gate] = filament;
        }
    }

    /// `MMU_SPOOLMAN` (`commands/mmu_spoolman.py`): with `off` it only logs that Spoolman is disabled (the command
    /// does not fail). `REFRESH=1` and `SYNC=1` give every gate with a spool the values of that spool again. `GATE=`
    /// with `SPOOLID=` assigns a spool in Spoolman's database, `GATE=` alone empties the gate, `SPOOLID=` alone takes
    /// the spool from its gate; the gate map follows in `pull` only (the other modes call `_spoolman_set_spool_gate`
    /// without `sync`). Without any parameter `readonly` logs that the gate map is not available.
    fn spoolman_command(
        &mut self,
        gate: Option<usize>,
        spool_id: Option<i64>,
        resync: bool,
    ) -> Result<(), CommandError> {
        if self.spoolman == SpoolmanMode::Off {
            self.console.push(format!("{ERROR_PREFIX}Spoolman support is currently disabled"));
            return Ok(());
        }
        // Klipper's `gcmd.get_int('SPOOLID', None, minval=1)`; the wording is assumed.
        if spool_id.is_some_and(|id| id < 1) {
            return Err(CommandError::new("SPOOLID must have minimum of 1"));
        }
        let spool = spool_id.and_then(|id| u32::try_from(id).ok());
        if let Some(gate) = gate {
            self.check_gate(gate)?;
        }
        if resync {
            for gate in 0..self.gates {
                self.take_filament_of_spool(gate);
            }
        }
        match (gate, spool) {
            (Some(gate), Some(spool)) => self.assign_in_spoolman(gate, spool),
            (Some(gate), None) => self.unassign_in_spoolman(self.gate_spool[gate], gate),
            (None, Some(spool)) => {
                if let Some(gate) = self.gate_spool.iter().position(|held| *held == Some(spool)) {
                    self.unassign_in_spoolman(Some(spool), gate);
                }
            }
            (None, None) if !resync && self.spoolman == SpoolmanMode::Readonly => self.console.push(format!(
                "{ERROR_PREFIX}Spoolman gate map not available. Spoolman mode is: {}",
                self.spoolman.as_str()
            )),
            (None, None) => {}
        }
        Ok(())
    }

    fn assign_in_spoolman(&mut self, gate: usize, spool: u32) {
        let pull = self.spoolman == SpoolmanMode::Pull;
        if let Some(old) = self.gate_spool[gate].filter(|old| pull && *old != spool) {
            self.log_unassigned(old, gate);
        }
        self.console.push(format!(
            "{INFO_PREFIX}Spool {spool} assigned to printer {PRINTER_HOSTNAME} @ gate {gate} in Spoolman db"
        ));
        if pull {
            self.assign_spool_id(gate, Some(spool));
            self.take_filament_of_spool(gate);
        }
    }

    fn unassign_in_spoolman(&mut self, spool: Option<u32>, gate: usize) {
        let Some(spool) = spool else { return };
        self.log_unassigned(spool, gate);
        if self.spoolman == SpoolmanMode::Pull {
            self.gate_spool[gate] = None;
        }
    }

    fn log_unassigned(&mut self, spool: u32, gate: usize) {
        self.console.push(format!(
            "{INFO_PREFIX}Spool {spool} unassigned from printer {PRINTER_HOSTNAME} and gate {gate} in Spoolman db"
        ));
    }

    fn per_gate<T>(&self, value: impl Fn(usize) -> T) -> Vec<T> {
        (0..self.gates).map(value).collect()
    }

    fn active_filament(&self) -> Value {
        match usize::try_from(self.gate) {
            Ok(gate) => {
                let filament = &self.gate_filament[gate];
                json!({
                    "filament_name": filament.name,
                    "material": filament.material,
                    "vendor": filament.vendor,
                    "color": filament.color,
                    "spool_id": self.gate_spool.get(gate).copied().flatten().map_or(-1, i64::from),
                    "temperature": filament.temperature,
                    "td": null,
                })
            }
            Err(_) => json!({}),
        }
    }

    fn mmu_json(&self, print_state: &str) -> Value {
        let printing = print_state == "printing";
        let paused = print_state == "paused";
        let mut mmu = json!({
            "enabled": true,
            "num_gates": self.gates,
            "is_homed": self.homed,
            "print_state": print_state,
            "unit": self.unit_json(),
            "tool": self.tool,
            "gate": self.gate,
            "active_filament": self.active_filament(),
            "num_toolchanges": self.toolchanges,
            "operation": "",
            "filament": if self.loaded { "Loaded" } else { "Unloaded" },
            "filament_pos": self.filament_position(),
            "filament_direction": self.direction,
            "bowden_progress": self.bowden_progress(),
            "filament_position": PATH_LENGTH * f64::from(self.filament_position().max(0)) / f64::from(FILAMENT_POSITION_LOADED)
                * f64::from(if self.direction < 0 { -1 } else { 1 }),
            "action": "Idle",
            // The fake states a reason for every pause; `pause_hint` is Happy Hare 4.2 and newer.
            "reason_for_pause": if paused { "Runout on gate 1" } else { "" },
            "pause_hint": if paused { "Load a new spool into gate 1, then RESUME" } else { "" },
            "is_paused": paused,
            "is_locked": false,
            "sensors": self.sensors_json(printing),
            "sync_drive": false,
            "ttg_map": self.tool_to_gate,
            "endless_spool_enabled": u8::from(self.endless_spool),
            "endless_spool_groups": self.endless_spool_groups,
            "gate_status": self.gate_status,
            "gate_filament_name": self.per_gate(|gate| self.gate_filament[gate].name.clone()),
            "gate_material": self.per_gate(|gate| self.gate_filament[gate].material.clone()),
            "gate_vendor": self.per_gate(|gate| self.gate_filament[gate].vendor.clone()),
            "gate_color": self.per_gate(|gate| self.gate_filament[gate].color.clone()),
            "gate_color_rgb": self.per_gate(|gate| self.gate_filament[gate].rgb),
            "gate_temperature": self.per_gate(|gate| self.gate_filament[gate].temperature),
            "gate_spool_id": self.gate_spool.iter().map(|id| id.map_or(-1, i64::from)).collect::<Vec<_>>(),
            "spoolman_support": self.spoolman.as_str(),
            "pending_spool_id": -1,
            "encoder": self.encoder_json(printing),
            "selector": { "has_bypass": self.selected_unit_has_bypass(), "servo": self.servo },
        });
        let (Some(fields), Value::Object(buffer)) = (mmu.as_object_mut(), self.buffer_fields(printing)) else {
            unreachable!("both are JSON objects")
        };
        fields.extend(buffer);
        if let Some(unit) = self.units.get(self.unit_selected) {
            // `mmu_controller.py` L830-860 merges the extruder wrapper of the active unit into `mmu`.
            if let Value::Object(wrapper) = extruder_wrapper_json(unit.extruder) {
                fields.extend(wrapper);
            }
        }
        mmu
    }

    /// `mmu.unit`; the older profiles show 0 and, before any gate, -1.
    fn unit_json(&self) -> i64 {
        if self.has_units() {
            self.unit_selected as i64
        } else if self.gate == NONE_SELECTED {
            i64::from(NONE_SELECTED)
        } else {
            0
        }
    }

    fn selected_unit_has_bypass(&self) -> bool {
        self.units.get(self.unit_selected).is_some_and(|unit| vendor_profile(unit.vendor).has_bypass)
    }

    /// The sync-feedback fields of the buffer, made up from the print state. Proportional as on the real device;
    /// the switch variants and the machine without buffer follow `happy-hare.md` (variants, Encoder). Without a
    /// buffer all `sync_feedback_*` and `tangle_prevention` are `None` and an encoder leaves `flowguard` shortened
    /// to `active`, `enabled` and `encoder_mode` (`unit/mmu_sync_feedback.py` L437-446 at c176ecc).
    fn buffer_fields(&self, printing: bool) -> Value {
        let Some(buffer) = self.buffer(printing) else {
            let flowguard = self.encoder.then(|| {
                // Assumption: `encoder_mode` is a boolean; the reference names the key but not the type.
                json!({ "active": printing, "enabled": printing, "encoder_mode": true })
            });
            return json!({
                "sync_feedback_enabled": null,
                "sync_feedback_state": null,
                "sync_feedback_bias_raw": null,
                "sync_feedback_bias_modelled": null,
                "sync_feedback_flow_rate": null,
                "flowguard": flowguard,
                "tangle_prevention": null,
            });
        };
        let mut flowguard = json!({
            "enabled": true,
            "active": printing,
            "trigger": "",
            "reason": "",
            "level": buffer.flowguard_level,
            "max_clog": buffer.flowguard_level.max(0.0),
            "max_tangle": buffer.flowguard_level.min(0.0),
        });
        if self.encoder {
            flowguard["encoder_mode"] = json!(true);
        }
        json!({
            "sync_feedback_enabled": 1,
            "sync_feedback_state": buffer.state,
            "sync_feedback_bias_raw": buffer.bias,
            "sync_feedback_bias_modelled": buffer.bias,
            "sync_feedback_flow_rate": buffer.flow_rate,
            "flowguard": flowguard,
            // Tangle prevention only acts with a proportional sensor: the dict exists for the switches, `active` stays
            // false there.
            "tangle_prevention": {
                "enabled": true,
                "active": printing && self.buffer == BufferKind::Proportional,
                "boosted": printing && self.boosting && self.buffer == BufferKind::Proportional,
                "threshold": TANGLE_THRESHOLD,
                "release": TANGLE_RELEASE,
            },
        })
    }

    /// The buffer in the three situations the UI tells apart: idle, printing in the middle, and printing with the
    /// filament at the stop (`FAKE_MMU_TANGLE`). A switch reports only its own side: the neutral state is never the
    /// opposite side (`mmu_sync_feedback.py` L174-181), the raw bias is 0 or +/-1, the flow rate stays at its start
    /// value 100. The modelled bias equals the raw one here (assumption; Happy Hare estimates it while printing).
    /// Values are made up and simplified. `None` without a buffer.
    fn buffer(&self, printing: bool) -> Option<Buffer> {
        let switch = |state, bias, flowguard_level| Buffer { state, bias, flow_rate: 100.0, flowguard_level };
        let stressed = printing && self.boosting;
        Some(match (self.buffer, printing, stressed) {
            (BufferKind::None, ..) => return None,
            (_, false, _) => Buffer { state: "inactive", bias: 0.0, flow_rate: 100.0, flowguard_level: 0.0 },
            (BufferKind::Proportional, true, false) => {
                Buffer { state: "neutral", bias: 0.05, flow_rate: 98.47, flowguard_level: 0.0 }
            }
            (BufferKind::Proportional, true, true) => {
                Buffer { state: "tension", bias: -0.93, flow_rate: 98.47, flowguard_level: -0.4 }
            }
            (_, true, false) => switch("neutral", 0.0, 0.0),
            (BufferKind::Compression, true, true) => switch("compressed", 1.0, 0.4),
            (_, true, true) => switch("tension", -1.0, -0.4),
        })
    }

    /// `mmu.encoder` (`unit/mmu_encoder.py` L471-481 at c176ecc), `None` without an encoder. The numbers are made
    /// up (assumptions): a 15 mm detection window in automatic mode, the headroom full when idle, part used while
    /// printing and nearly used up with the filament at the stop; the stop lets the flow rate fall.
    fn encoder_json(&self, printing: bool) -> Value {
        if !self.encoder {
            return Value::Null;
        }
        const DETECTION_LENGTH: f64 = 15.0;
        let (headroom, flow_rate) = match (printing, self.boosting) {
            (false, _) => (DETECTION_LENGTH, 100.0),
            (true, false) => (11.0, 99.2),
            (true, true) => (2.5, 91.0),
        };
        json!({
            "encoder_pos": PATH_LENGTH * f64::from(self.filament_position().max(0)) / f64::from(FILAMENT_POSITION_LOADED),
            "detection_length": DETECTION_LENGTH,
            "headroom": headroom,
            "min_headroom": 0.0,
            "desired_headroom": 6.0,
            "detection_mode": 2,
            "enabled": printing,
            "flow_rate": flow_rate,
        })
    }

    /// `mmu.sensors` with generic names; the Happy Hare 3 aliases are included as Happy Hare 4.1 does for older UIs.
    fn sensors_json(&self, printing: bool) -> Value {
        let at_entry = usize::try_from(self.gate)
            .ok()
            .and_then(|gate| self.gate_status.get(gate))
            .is_some_and(|status| *status == GATE_AVAILABLE);
        let past = |sensor: i32| self.filament_position() >= sensor;
        let mut sensors = json!({
            "mmu_entry": at_entry,
            "mmu_exit": past(FILAMENT_POSITION_GATE_SENSOR),
            "extruder": past(FILAMENT_POSITION_EXTRUDER_SENSOR),
            "toolhead": past(FILAMENT_POSITION_TOOLHEAD_SENSOR),
            "mmu_pre_gate": at_entry,
            "mmu_gear": past(FILAMENT_POSITION_GATE_SENSOR),
        });
        // The buffer variant shows in which keys exist (a missing key is not fitted); only a proportional sensor
        // has `filament_proportional`, then with the virtual switches (`unit/mmu_buffer.py` L90-103).
        let stressed = printing && self.boosting;
        let sensor_map = sensors.as_object_mut().expect("sensors is built by json! as an object above");
        match self.buffer {
            BufferKind::Proportional => {
                sensor_map.insert(SENSOR_COMPRESSION.to_owned(), json!(false));
                sensor_map.insert(SENSOR_TENSION.to_owned(), json!(stressed));
                sensor_map.insert(SENSOR_PROPORTIONAL.to_owned(), json!(false));
            }
            BufferKind::Compression => {
                sensor_map.insert(SENSOR_COMPRESSION.to_owned(), json!(stressed));
            }
            BufferKind::Tension => {
                sensor_map.insert(SENSOR_TENSION.to_owned(), json!(stressed));
            }
            BufferKind::None => {}
        }
        if self.encoder {
            // "Movement detected", not "filament present" (`mmu_encoder.py`): true while the filament runs.
            sensor_map.insert(SENSOR_ENCODER.to_owned(), json!(printing));
        }
        // The active map holds the sensors of the selected gate's unit, its shared exit too (`mmu_sensor_manager.py`
        // L190-206, generic names without owner).
        let selected = usize::try_from(self.gate).ok();
        let unit = self
            .sensor_units()
            .into_iter()
            .find(|unit| selected.is_some_and(|gate| (unit.first_gate..unit.first_gate + unit.gates).contains(&gate)));
        if unit.is_some_and(|unit| unit.shared_exit.is_some()) {
            sensors[SENSOR_SHARED_EXIT] = json!(past(FILAMENT_POSITION_GATE_SENSOR));
        }
        sensors
    }

    /// The Klipper objects `filament_switch_sensor <name>` of the machine with their `filament_detected`, named like
    /// `MmuSensorManager` and the sensor classes do (`mmu_sensor_manager.py` L23-90): gate sensors with the global
    /// gate number and no prefix, the others with their owner (unit, toolhead or buffer) in front, which is dropped
    /// with exactly one unit. The older profiles have one unit like the recorded single-lane type-B machine: gate 0
    /// only, a buffer and the toolhead. The sensors follow the simulated filament like `mmu.sensors`; entry sensors
    /// stay on while their gate holds filament, the others only for the unit of the selected gate.
    pub fn filament_sensors(&self, print_state: &str) -> Vec<(String, bool)> {
        let printing = print_state == "printing";
        let selected_gate = usize::try_from(self.gate).ok();
        let past = |sensor: i32| self.filament_position() >= sensor;
        // `mmu_sensor_utils.py` L363-368: the registered name loses the owner prefix with exactly one unit.
        let owned = |owner: &str, sensor: &str| {
            let name = if self.units.len() >= 2 { format!("{owner}:{sensor}") } else { sensor.to_owned() };
            format!("{SENSOR_PREFIX}{name}")
        };
        let mut sensors: Vec<(String, bool)> = Vec::new();
        let mut add = |name: String, detected: bool| {
            // Units may share a toolhead or a buffer (`mmu_unit.py` L463-473, L534-546): one sensor, which sees the
            // filament of any of them.
            match sensors.iter_mut().find(|(known, _)| *known == name) {
                Some((_, known_detected)) => *known_detected |= detected,
                None => sensors.push((name, detected)),
            }
        };
        for unit in self.sensor_units() {
            let gates = unit.first_gate..unit.first_gate + unit.gates;
            let active = selected_gate.is_some_and(|gate| gates.contains(&gate));
            for gate in gates {
                let holds_filament = self.gate_status.get(gate) == Some(&GATE_AVAILABLE);
                add(format!("{SENSOR_PREFIX}{SENSOR_ENTRY}_{gate}"), holds_filament);
                add(
                    format!("{SENSOR_PREFIX}{SENSOR_EXIT}_{gate}"),
                    selected_gate == Some(gate) && past(FILAMENT_POSITION_GATE_SENSOR),
                );
            }
            if let Some(owner) = unit.shared_exit {
                add(owned(owner, SENSOR_SHARED_EXIT), active && past(FILAMENT_POSITION_GATE_SENSOR));
            }
            // The extruder and toolhead sensors belong to the `[mmu_toolhead]` (`mmu_toolhead_wrapper.py` L86, L102).
            // Same order as the recorded machine lists them (toolhead before extruder).
            add(owned(unit.toolhead, SENSOR_TOOLHEAD), active && past(FILAMENT_POSITION_TOOLHEAD_SENSOR));
            add(owned(unit.toolhead, SENSOR_EXTRUDER), active && past(FILAMENT_POSITION_EXTRUDER_SENSOR));
            if let Some(buffer) = unit.buffer {
                // `mmu_buffer.py` L64, L77: compression and tension belong to the buffer.
                let stressed = active && printing && self.boosting;
                if matches!(self.buffer, BufferKind::Proportional | BufferKind::Compression) {
                    add(owned(buffer, SENSOR_COMPRESSION), self.buffer == BufferKind::Compression && stressed);
                }
                if matches!(self.buffer, BufferKind::Proportional | BufferKind::Tension) {
                    add(owned(buffer, SENSOR_TENSION), stressed);
                }
            }
        }
        sensors
    }

    /// The units as far as their sensors go: the units of the profile, or the one legacy unit.
    fn sensor_units(&self) -> Vec<SensorUnit> {
        if !self.has_units() {
            return vec![SensorUnit {
                first_gate: 0,
                gates: 1,
                shared_exit: None,
                toolhead: "default",
                buffer: Some(UNIT_NAME),
            }];
        }
        self.units
            .iter()
            .zip(self.first_gates())
            .map(|(unit, first_gate)| SensorUnit {
                first_gate,
                gates: unit.gates,
                shared_exit: vendor_profile(unit.vendor).shared_exit_sensor.then_some(unit.name),
                toolhead: unit.toolhead,
                buffer: unit.buffer,
            })
            .collect()
    }

    /// `mmu_machine` of a machine with units: `happy_hare_version`, `num_units`, `num_gates` and a `unit_<i>` each,
    /// with what `MmuUnit.get_status` returns (`mmu_unit.py` L1086-1145) for a unit without fans, heaters, NFC
    /// readers or TD-1 scanners (`mmu_machine.py` L93-131, L179).
    fn units_machine_json(&self) -> Value {
        let mut machine = Map::new();
        machine.insert("happy_hare_version".to_owned(), json!(VERSION));
        machine.insert("num_units".to_owned(), json!(self.units.len()));
        machine.insert("num_gates".to_owned(), json!(self.gates));
        for (index, (unit, first_gate)) in self.units.iter().zip(self.first_gates()).enumerate() {
            let profile = vendor_profile(unit.vendor);
            machine.insert(
                format!("unit_{index}"),
                json!({
                    "name": unit.name,
                    "display_name": unit.display_name,
                    "vendor": unit.vendor,
                    "version": unit.version,
                    "num_gates": unit.gates,
                    "first_gate": first_gate,
                    "selector_type": "LinearServoSelector",
                    "is_homed": self.homed,
                    "variable_rotation_distances": profile.variable_rotation_distances,
                    "variable_bowden_lengths": false,
                    "require_bowden_move": true,
                    "filament_always_gripped": false,
                    "has_bypass": profile.has_bypass,
                    "can_crossload": false,
                    "multi_gear": false,
                    "filament_buffer": true,
                }),
            );
        }
        Value::Object(machine)
    }

    fn machine_json(&self) -> Value {
        if self.has_units() {
            return self.units_machine_json();
        }
        json!({
            "happy_hare_version": VERSION,
            "num_units": 1,
            "num_gates": self.gates,
            "unit_0": {
                "name": UNIT_NAME,
                "display_name": "Unit 0",
                "vendor": "ERCF",
                "version": "1.1sb",
                "num_gates": self.gates,
                "first_gate": 0,
                "selector_type": "LinearServoSelector",
                "is_homed": self.homed,
                "has_bypass": false,
                "can_crossload": false,
                "multi_gear": false,
                "filament_buffer": false,
            },
        })
    }
}

/// `mmu_extruder <name>` (`mmu_extruder_wrapper.py` L218-227) of an extruder that has not moved any filament yet:
/// the values of L36-39. The same keys end up in `mmu` for the active unit.
fn extruder_wrapper_json(extruder: &str) -> Value {
    json!({
        "extruder_name": extruder,
        "extruder_filament_remaining": 0.0,
        "filament_remaining": 0.0,
        "filament_remaining_color": UNKNOWN_FILAMENT_COLOR,
        "filament_remaining_material": "",
        "filament_remaining_temp": 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: &str = "standby";

    fn homed(gates: usize) -> HappyHare {
        let mut hare = HappyHare::new(gates, SpoolmanMode::Push);
        hare.apply(MmuCommand::Home, IDLE).unwrap();
        hare
    }

    fn mmu(hare: &HappyHare) -> Value {
        hare.status("standby")["mmu"].clone()
    }

    #[test]
    fn starts_unhomed_with_nothing_selected() {
        let status = mmu(&HappyHare::new(4, SpoolmanMode::Push));

        assert_eq!(status["is_homed"], false);
        assert_eq!(status["gate"], -1);
        assert_eq!(status["tool"], -1);
        assert_eq!(status["filament"], "Unloaded");
        assert_eq!(status["active_filament"], json!({}));
    }

    #[test]
    fn last_gate_is_empty() {
        let status = mmu(&HappyHare::new(4, SpoolmanMode::Push));

        assert_eq!(status["gate_status"], json!([1, 1, 1, 0]));
        assert_eq!(status["gate_filament_name"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn every_move_requires_homing() {
        let mut hare = HappyHare::new(4, SpoolmanMode::Push);

        for command in [MmuCommand::Select { gate: 1 }, MmuCommand::ChangeTool { tool: 1 }, MmuCommand::Load] {
            assert_eq!(hare.apply(command, IDLE).unwrap_err().message(), "MMU is not homed");
        }
    }

    #[test]
    fn select_sets_gate_and_mapped_tool() {
        let mut hare = homed(4);

        hare.apply(MmuCommand::Select { gate: 2 }, IDLE).unwrap();

        let status = mmu(&hare);
        assert_eq!(status["gate"], 2);
        assert_eq!(status["tool"], 2);
        assert_eq!(status["unit"], 0);
        // Gate 2 holds the spool with three colours (spool 8), whose first colour Happy Hare reports.
        assert_eq!(status["active_filament"]["filament_name"], "Silk Red+Gold+Black");
        assert_eq!(status["active_filament"]["spool_id"], 8);
    }

    #[test]
    fn the_bypass_is_selected_and_loaded_unless_filament_is_loaded() {
        let mut hare = homed(4);

        hare.apply(MmuCommand::SelectBypass, IDLE).unwrap();
        hare.apply(MmuCommand::Load, IDLE).unwrap();

        let status = mmu(&hare);
        assert_eq!((status["gate"].clone(), status["tool"].clone()), (json!(-2), json!(-2)));
        assert_eq!(status["filament"], "Loaded");
        assert!(hare.apply(MmuCommand::SelectBypass, IDLE).is_err());
    }

    #[test]
    fn a_tool_is_mapped_to_another_gate_and_back() {
        let mut hare = homed(4);
        hare.apply(MmuCommand::Select { gate: 3 }, IDLE).unwrap();

        hare.apply(MmuCommand::MapTool { tool: 1, gate: 3 }, IDLE).unwrap();
        let status = mmu(&hare);
        assert_eq!(status["ttg_map"], json!([0, 3, 2, 3]));
        assert_eq!(status["gate_status"][3], GATE_UNKNOWN, "the empty gate was probably just filled");
        assert_eq!(status["tool"], 1);

        hare.apply(MmuCommand::ResetToolMap, IDLE).unwrap();
        assert_eq!(mmu(&hare)["ttg_map"], json!([0, 1, 2, 3]));
        assert_eq!(mmu(&hare)["tool"], 3);
    }

    #[test]
    fn endless_spool_takes_one_group_per_gate() {
        let mut hare = homed(3);

        hare.apply(MmuCommand::EndlessSpool { enabled: Some(true), groups: Some(vec![0, 0, 1]) }, IDLE).unwrap();
        assert!(hare.apply(MmuCommand::EndlessSpool { enabled: None, groups: Some(vec![0]) }, IDLE).is_err());

        let status = mmu(&hare);
        assert_eq!(status["endless_spool_enabled"], 1);
        assert_eq!(status["endless_spool_groups"], json!([0, 0, 1]));
    }

    #[test]
    fn change_tool_loads_filament_and_counts() {
        let mut hare = homed(4);

        hare.apply(MmuCommand::ChangeTool { tool: 1 }, IDLE).unwrap();
        hare.apply(MmuCommand::ChangeTool { tool: 2 }, IDLE).unwrap();

        let status = mmu(&hare);
        assert_eq!(status["tool"], 2);
        assert_eq!(status["gate"], 2);
        assert_eq!(status["filament"], "Loaded");
        assert_eq!(status["filament_pos"], 10);
        assert_eq!(status["num_toolchanges"], 2);
    }

    #[test]
    fn empty_gate_cannot_be_used() {
        let mut hare = homed(4);

        let error = hare.apply(MmuCommand::ChangeTool { tool: 3 }, IDLE).unwrap_err();

        assert_eq!(error.message(), "Gate 3 is empty");
        assert_eq!(mmu(&hare)["filament"], "Unloaded");
    }

    #[test]
    fn unknown_tool_and_gate_are_rejected() {
        let mut hare = homed(4);

        assert_eq!(
            hare.apply(MmuCommand::ChangeTool { tool: 9 }, IDLE).unwrap_err().message(),
            "Tool T9 does not exist"
        );
        assert_eq!(hare.apply(MmuCommand::Select { gate: 9 }, IDLE).unwrap_err().message(), "Gate 9 does not exist");
    }

    #[test]
    fn gate_cannot_change_while_loaded() {
        let mut hare = homed(4);
        hare.apply(MmuCommand::ChangeTool { tool: 0 }, IDLE).unwrap();

        let error = hare.apply(MmuCommand::Select { gate: 1 }, IDLE).unwrap_err();

        assert_eq!(error.message(), "Cannot change gate while filament is loaded");
    }

    #[test]
    fn unload_keeps_the_gate_selected() {
        let mut hare = homed(4);
        hare.apply(MmuCommand::ChangeTool { tool: 1 }, IDLE).unwrap();

        hare.apply(MmuCommand::Unload, IDLE).unwrap();

        let status = mmu(&hare);
        assert_eq!(status["filament"], "Unloaded");
        assert_eq!(status["gate"], 1);
        assert!(hare.apply(MmuCommand::Unload, IDLE).is_err());
    }

    #[test]
    fn eject_unloads_first_and_then_empties_the_gate() {
        let mut hare = homed(4);
        hare.apply(MmuCommand::Select { gate: 1 }, IDLE).unwrap();
        hare.apply(MmuCommand::Load, IDLE).unwrap();

        hare.apply(MmuCommand::Eject { gate: None, force: false }, IDLE).unwrap();
        assert_eq!(mmu(&hare)["filament"], "Unloaded");
        assert_eq!(mmu(&hare)["gate_status"][1], GATE_AVAILABLE);

        hare.apply(MmuCommand::Eject { gate: None, force: false }, IDLE).unwrap();
        assert_eq!(mmu(&hare)["gate_status"][1], GATE_EMPTY);
        assert_eq!(
            hare.apply(MmuCommand::Eject { gate: Some(9), force: false }, IDLE).unwrap_err().message(),
            "Gate 9 does not exist"
        );
    }

    #[test]
    fn a_gate_map_sets_the_spool_and_the_gate_takes_its_filament() {
        let mut hare = homed(4);
        assert_eq!(mmu(&hare)["gate_spool_id"], json!([1, 7, 8, -1]));

        hare.apply(spool_map(1, 4), IDLE).unwrap();
        hare.apply(spool_map(0, -1), IDLE).unwrap();

        let status = mmu(&hare);
        assert_eq!(status["gate_spool_id"], json!([-1, 4, 8, -1]));
        // Spool 4 is the black ABS: Happy Hare takes material and temperature from Spoolman.
        assert_eq!(status["gate_material"][1], "ABS");
        assert_eq!(status["spoolman_support"], "push");
        assert!(hare.apply(spool_map(9, 1), IDLE).is_err());
    }

    fn spool_map(gate: usize, spool_id: i64) -> MmuCommand {
        gate_map(gate, Some(spool_id))
    }

    fn gate_map(gate: usize, spool_id: Option<i64>) -> MmuCommand {
        MmuCommand::GateMap {
            gate,
            spool_id,
            name: None,
            material: None,
            vendor: None,
            color: None,
            temperature: None,
            weights_given: false,
        }
    }

    #[test]
    fn without_spoolman_the_gate_map_keeps_material_colour_and_temperature() {
        let mut hare = HappyHare::new(4, SpoolmanMode::Off);
        let set = |material: Option<&str>, color: Option<&str>| MmuCommand::GateMap {
            gate: 1,
            spool_id: None,
            name: None,
            material: material.map(str::to_owned),
            vendor: None,
            color: color.map(str::to_owned),
            temperature: Some(240),
            weights_given: false,
        };

        hare.apply(set(Some("PETG"), None), IDLE).unwrap();
        hare.apply(set(None, Some("ff8000")), IDLE).unwrap();

        let status = mmu(&hare);
        assert_eq!(status["spoolman_support"], "off");
        assert_eq!(status["gate_spool_id"], json!([-1, -1, -1, -1]));
        assert_eq!((&status["gate_material"][1], &status["gate_color"][1]), (&json!("PETG"), &json!("ff8000")));
        assert_eq!(status["gate_color_rgb"][1], json!([1.0, 128.0 / 255.0, 0.0]));
        assert_eq!(status["gate_temperature"][1], 240);
        assert!(hare.apply(set(None, Some("orange")), IDLE).is_err());
    }

    fn homed_in(mode: SpoolmanMode) -> HappyHare {
        let mut hare = HappyHare::new(4, mode);
        hare.apply(MmuCommand::Home, IDLE).unwrap();
        hare
    }

    fn spoolman_command(gate: Option<usize>, spool_id: Option<i64>, refresh: bool) -> MmuCommand {
        MmuCommand::Spoolman { gate, spool_id, refresh, sync: false }
    }

    fn gate_map_with(gate: usize, material: Option<&str>, temperature: Option<i32>, weights_given: bool) -> MmuCommand {
        MmuCommand::GateMap {
            gate,
            spool_id: None,
            name: None,
            material: material.map(str::to_owned),
            vendor: None,
            color: None,
            temperature,
            weights_given,
        }
    }

    #[test]
    fn a_load_makes_the_spool_of_the_gate_the_active_one_and_an_unload_clears_it() {
        let mut hare = homed(4);
        hare.apply(MmuCommand::Select { gate: 1 }, IDLE).unwrap();
        assert!(hare.take_active_spool_events().is_empty(), "selecting a gate does not tell Moonraker");

        hare.apply(MmuCommand::Load, IDLE).unwrap();
        assert_eq!(hare.take_active_spool_events(), [Some(7)]);

        hare.apply(MmuCommand::Unload, IDLE).unwrap();
        assert_eq!(hare.take_active_spool_events(), [None]);
    }

    #[test]
    fn a_tool_change_unloads_the_old_spool_before_it_loads_the_new_one() {
        let mut hare = homed(4);
        hare.apply(MmuCommand::ChangeTool { tool: 0 }, IDLE).unwrap();
        hare.apply(MmuCommand::ChangeTool { tool: 1 }, IDLE).unwrap();

        assert_eq!(hare.take_active_spool_events(), [Some(1), None, Some(7)]);
    }

    #[test]
    fn a_gate_without_a_spool_sets_no_active_spool_and_off_never_does() {
        let mut hare = homed(4);
        hare.apply(spool_map(1, -1), IDLE).unwrap();
        hare.apply(MmuCommand::Select { gate: 1 }, IDLE).unwrap();
        hare.apply(MmuCommand::Load, IDLE).unwrap();
        assert!(hare.take_active_spool_events().is_empty());

        let mut off = homed_in(SpoolmanMode::Off);
        off.apply(MmuCommand::ChangeTool { tool: 0 }, IDLE).unwrap();
        off.apply(MmuCommand::Unload, IDLE).unwrap();
        assert!(off.take_active_spool_events().is_empty());
    }

    #[test]
    fn a_new_spool_on_the_loaded_gate_is_not_the_active_one_until_the_next_load() {
        let mut hare = homed(4);
        hare.apply(MmuCommand::ChangeTool { tool: 0 }, IDLE).unwrap();
        hare.take_active_spool_events();

        hare.apply(spool_map(0, 4), IDLE).unwrap();
        assert!(hare.take_active_spool_events().is_empty());

        hare.apply(MmuCommand::Unload, IDLE).unwrap();
        hare.apply(MmuCommand::Load, IDLE).unwrap();
        assert_eq!(hare.take_active_spool_events(), [None, Some(4)]);
    }

    #[test]
    fn the_gate_values_are_a_copy_that_differs_from_the_spool_of_the_second_gate_until_a_refresh() {
        let mut hare = homed(4);
        let status = mmu(&hare);
        // Spool 7 is the PLA gradient; someone edited it in Spoolman after Happy Hare took the values.
        assert_eq!(status["gate_material"], json!(["PLA", "PETG", "PLA", "ABS"]));
        assert_eq!(status["gate_color"][1], EDITED_COLOR);

        hare.apply(spoolman_command(None, None, true), IDLE).unwrap();

        let status = mmu(&hare);
        assert_eq!(status["gate_material"][1], "PLA");
        // A spool with several colours has no `color_hex`, so the gate has no colour.
        assert_eq!(status["gate_color"][1], "");
        assert_eq!(status["gate_color_rgb"][1], json!([0.0, 0.0, 0.0]));
        assert_eq!(status["gate_temperature"][1], 215);
        assert!(hare.take_console().is_empty());
    }

    #[test]
    fn a_changed_spool_id_fetches_the_spool_but_the_same_id_only_takes_the_other_parameters() {
        let mut hare = homed(4);
        hare.apply(spool_map(0, 4), IDLE).unwrap();
        let status = mmu(&hare);
        assert_eq!((&status["gate_material"][0], &status["gate_temperature"][0]), (&json!("ABS"), &json!(250)));
        assert_eq!(status["gate_spool_id"], json!([4, 7, 8, -1]));

        let mut same = gate_map_with(0, Some("ASA"), Some(255), false);
        if let MmuCommand::GateMap { spool_id, .. } = &mut same {
            *spool_id = Some(4);
        }
        hare.apply(same, IDLE).unwrap();

        let status = mmu(&hare);
        assert_eq!((&status["gate_material"][0], &status["gate_temperature"][0]), (&json!("ASA"), &json!(255)));
    }

    #[test]
    fn a_spool_sits_on_one_gate_only_and_minus_one_keeps_the_values() {
        let mut hare = homed(4);

        hare.apply(spool_map(3, 1), IDLE).unwrap();
        assert_eq!(mmu(&hare)["gate_spool_id"], json!([-1, 7, 8, 1]));

        hare.apply(spool_map(3, -1), IDLE).unwrap();
        let status = mmu(&hare);
        assert_eq!(status["gate_spool_id"], json!([-1, 7, 8, -1]));
        assert_eq!(status["gate_material"][3], "PLA");
    }

    #[test]
    fn with_spoolman_off_the_spool_id_is_only_stored() {
        let mut hare = homed_in(SpoolmanMode::Off);
        assert_eq!(mmu(&hare)["gate_spool_id"], json!([-1, -1, -1, -1]));

        hare.apply(spool_map(0, 4), IDLE).unwrap();

        let status = mmu(&hare);
        assert_eq!(status["gate_spool_id"], json!([4, -1, -1, -1]));
        assert_eq!(status["gate_material"][0], "PLA", "nothing is fetched");
    }

    #[test]
    fn spoolman_off_refuses_the_spoolman_command_in_the_console_without_failing() {
        let mut hare = homed_in(SpoolmanMode::Off);

        hare.apply(spoolman_command(Some(0), Some(4), false), IDLE).unwrap();

        assert_eq!(hare.take_console(), ["!! Spoolman support is currently disabled"]);
    }

    #[test]
    fn pull_refuses_the_parameters_of_spoolman_in_the_console_and_does_not_fail() {
        let mut hare = homed_in(SpoolmanMode::Pull);
        assert_eq!(mmu(&hare)["spoolman_support"], "pull");

        for command in
            [spool_map(0, 4), gate_map_with(0, Some("PETG"), None, false), gate_map_with(0, None, None, true)]
        {
            hare.apply(command, IDLE).unwrap();
            assert_eq!(
                hare.take_console(),
                [
                    "!! Spoolman mode is 'pull': Can only set gate status and speed override locally\nUse MMU_SPOOLMAN or update spoolman directly"
                ]
            );
        }
        let status = mmu(&hare);
        assert_eq!(status["gate_spool_id"], json!([1, 7, 8, -1]));
        assert_eq!(status["gate_material"][0], "PLA");
    }

    #[test]
    fn pull_ignores_the_temperature_without_a_message() {
        let mut hare = homed_in(SpoolmanMode::Pull);

        hare.apply(gate_map_with(0, None, Some(300), false), IDLE).unwrap();

        assert!(hare.take_console().is_empty());
        assert_eq!(mmu(&hare)["gate_temperature"][0], 210);
    }

    #[test]
    fn pull_assigns_and_empties_a_gate_through_the_spoolman_command() {
        let mut hare = homed_in(SpoolmanMode::Pull);

        hare.apply(spoolman_command(Some(3), Some(4), false), IDLE).unwrap();
        let status = mmu(&hare);
        assert_eq!(status["gate_spool_id"], json!([1, 7, 8, 4]));
        assert_eq!(status["gate_material"][3], "ABS");
        assert_eq!(hare.take_console(), ["// Spool 4 assigned to printer fake-printer @ gate 3 in Spoolman db"]);

        hare.apply(spoolman_command(Some(3), Some(2), false), IDLE).unwrap();
        assert_eq!(
            hare.take_console(),
            [
                "// Spool 4 unassigned from printer fake-printer and gate 3 in Spoolman db",
                "// Spool 2 assigned to printer fake-printer @ gate 3 in Spoolman db",
            ]
        );

        hare.apply(spoolman_command(Some(3), None, false), IDLE).unwrap();
        assert_eq!(mmu(&hare)["gate_spool_id"], json!([1, 7, 8, -1]));
        assert_eq!(hare.take_console(), ["// Spool 2 unassigned from printer fake-printer and gate 3 in Spoolman db"]);
    }

    #[test]
    fn the_spoolman_command_needs_a_positive_spool_id() {
        let mut hare = homed_in(SpoolmanMode::Pull);

        let error = hare.apply(spoolman_command(Some(0), Some(-1), false), IDLE).unwrap_err();

        assert_eq!(error.message(), "SPOOLID must have minimum of 1");
    }

    #[test]
    fn push_and_readonly_leave_the_gate_map_to_the_spoolman_command() {
        for mode in [SpoolmanMode::Push, SpoolmanMode::Readonly] {
            let mut hare = homed_in(mode);

            hare.apply(spoolman_command(Some(3), Some(4), false), IDLE).unwrap();

            assert_eq!(mmu(&hare)["gate_spool_id"], json!([1, 7, 8, -1]));
            assert_eq!(hare.take_console().len(), 1);
        }
        let mut hare = homed_in(SpoolmanMode::Readonly);
        hare.apply(spoolman_command(None, None, false), IDLE).unwrap();
        assert_eq!(hare.take_console(), ["!! Spoolman gate map not available. Spoolman mode is: readonly"]);
    }

    #[test]
    fn load_needs_a_selected_gate() {
        let mut hare = homed(4);

        assert_eq!(hare.apply(MmuCommand::Load, IDLE).unwrap_err().message(), "No gate selected");

        hare.apply(MmuCommand::Select { gate: 1 }, IDLE).unwrap();
        hare.apply(MmuCommand::Load, IDLE).unwrap();
        assert_eq!(mmu(&hare)["filament"], "Loaded");
    }

    #[test]
    fn motors_off_loses_the_home() {
        let mut hare = homed(4);

        hare.apply(MmuCommand::MotorsOff, IDLE).unwrap();
        assert_eq!(mmu(&hare)["is_homed"], false);
    }

    #[test]
    fn a_check_marks_gates_with_filament_available() {
        let mut hare = homed(4);
        hare.gate_status = vec![-1, 1, 2, GATE_EMPTY];

        hare.apply(MmuCommand::CheckGate { gate: Some(0), all: false }, IDLE).unwrap();
        assert_eq!(mmu(&hare)["gate_status"], json!([1, 1, 2, 0]));

        hare.apply(MmuCommand::CheckGate { gate: None, all: true }, IDLE).unwrap();
        assert_eq!(mmu(&hare)["gate_status"], json!([1, 1, 1, 0]));
        assert_eq!(
            hare.apply(MmuCommand::CheckGate { gate: None, all: false }, IDLE).unwrap_err().message(),
            "Current gate is invalid"
        );
    }

    #[test]
    fn a_preload_fills_an_empty_gate_but_not_while_printing() {
        let mut hare = homed(4);

        assert!(hare.apply(MmuCommand::Preload { gate: Some(3) }, "printing").is_err());
        hare.apply(MmuCommand::Preload { gate: Some(3) }, IDLE).unwrap();
        assert_eq!(mmu(&hare)["gate_status"][3], GATE_AVAILABLE);

        hare.apply(MmuCommand::ChangeTool { tool: 1 }, IDLE).unwrap();
        assert_eq!(
            hare.apply(MmuCommand::Preload { gate: None }, IDLE).unwrap_err().message(),
            "Operation not possible. Filament is loaded"
        );
    }

    #[test]
    fn the_servo_moves_and_keeps_its_position_when_switched_off() {
        let mut hare = homed(4);
        assert_eq!(mmu(&hare)["selector"]["servo"], SERVO_UP);

        hare.apply(MmuCommand::Servo { position: Some(ServoPosition::Down) }, IDLE).unwrap();
        hare.apply(MmuCommand::Servo { position: Some(ServoPosition::Off) }, IDLE).unwrap();
        assert_eq!(mmu(&hare)["selector"]["servo"], SERVO_DOWN);
    }

    #[test]
    fn one_unit_generates_the_six_sensors_of_the_recorded_single_lane_machine() {
        let names: Vec<String> = HappyHare::new(4, SpoolmanMode::Off)
            .filament_sensors("standby")
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        assert_eq!(
            names,
            [
                "filament_switch_sensor mmu_entry_0",
                "filament_switch_sensor mmu_exit_0",
                "filament_switch_sensor toolhead",
                "filament_switch_sensor extruder",
                "filament_switch_sensor filament_compression",
                "filament_switch_sensor filament_tension",
            ]
        );
    }

    #[test]
    fn status_registers_all_happy_hare_objects() {
        let objects = HappyHare::new(4, SpoolmanMode::Push).status("standby");

        let names: Vec<&str> = objects.keys().map(String::as_str).collect();
        assert_eq!(names, ["mmu", "mmu_extruder extruder", "mmu_machine", "mmu_stepper unit0_gear"]);
        assert_eq!(objects["mmu_machine"]["unit_0"]["num_gates"], 4);
    }

    fn stressed_print(buffer: BufferKind, encoder: bool) -> Value {
        let mut hare = HappyHare::new(4, SpoolmanMode::Off).with_feedback(buffer, encoder);
        hare.apply(MmuCommand::FakeTangle { boosting: true }, IDLE).unwrap();
        hare.status("printing")["mmu"].clone()
    }

    #[test]
    fn a_compression_only_buffer_reports_only_its_own_side() {
        let printing = stressed_print(BufferKind::Compression, true);

        assert_eq!(printing["sync_feedback_state"], "compressed");
        assert_eq!(printing["sync_feedback_bias_raw"], 1.0);
        assert_eq!(printing["sync_feedback_flow_rate"], 100.0);
        assert_eq!(printing["sensors"]["filament_compression"], true);
        assert!(printing["sensors"].get("filament_tension").is_none());
        assert!(printing["sensors"].get("filament_proportional").is_none());
        assert_eq!(printing["tangle_prevention"]["active"], false);
        assert_eq!(printing["flowguard"]["level"], 0.4);
        let hare = HappyHare::new(4, SpoolmanMode::Off).with_feedback(BufferKind::Compression, false);
        assert_eq!(hare.status("printing")["mmu"]["sync_feedback_state"], "neutral");
    }

    #[test]
    fn a_tension_only_buffer_reports_only_its_own_side() {
        let printing = stressed_print(BufferKind::Tension, false);

        assert_eq!(printing["sync_feedback_state"], "tension");
        assert_eq!(printing["sync_feedback_bias_raw"], -1.0);
        assert_eq!(printing["sensors"]["filament_tension"], true);
        assert!(printing["sensors"].get("filament_compression").is_none());
        assert!(printing["sensors"].get("encoder").is_none());
        assert!(printing["encoder"].is_null());
        assert_eq!(printing["tangle_prevention"]["boosted"], false);
        let names: Vec<String> = HappyHare::new(4, SpoolmanMode::Off)
            .with_feedback(BufferKind::Tension, false)
            .filament_sensors("standby")
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert!(names.contains(&"filament_switch_sensor filament_tension".to_owned()));
        assert!(!names.contains(&"filament_switch_sensor filament_compression".to_owned()));
    }

    #[test]
    fn an_encoder_without_buffer_leaves_a_shortened_flowguard() {
        let hare = HappyHare::new(4, SpoolmanMode::Off).with_feedback(BufferKind::None, true);
        let idle = hare.status("standby")["mmu"].clone();
        let printing = stressed_print(BufferKind::None, true);

        assert!(printing["sync_feedback_state"].is_null());
        assert!(printing["tangle_prevention"].is_null());
        let flowguard = printing["flowguard"].as_object().unwrap();
        assert_eq!(flowguard.keys().map(String::as_str).collect::<Vec<_>>(), ["active", "enabled", "encoder_mode"]);
        assert_eq!(idle["encoder"].as_object().unwrap().len(), 8);
        assert_eq!(idle["encoder"]["headroom"], idle["encoder"]["detection_length"]);
        assert_eq!(idle["encoder"]["enabled"], false);
        assert_eq!(printing["encoder"]["enabled"], true);
        assert!(printing["encoder"]["headroom"].as_f64().unwrap() < 6.0);
        assert_eq!(printing["sensors"]["encoder"], true);
        assert!(printing["sensors"].get("filament_compression").is_none());
        let names: Vec<String> = hare.filament_sensors("standby").into_iter().map(|(name, _)| name).collect();
        assert!(!names.iter().any(|name| name.contains("compression") || name.contains("tension")));
    }

    #[test]
    fn the_buffer_is_idle_until_a_print_runs() {
        let hare = homed(4);

        assert_eq!(hare.status("standby")["mmu"]["sync_feedback_state"], "inactive");
        assert_eq!(hare.status("standby")["mmu"]["flowguard"]["active"], false);
        assert_eq!(hare.status("printing")["mmu"]["sync_feedback_state"], "neutral");
        assert_eq!(hare.status("printing")["mmu"]["flowguard"]["active"], true);
        assert_eq!(hare.status("printing")["mmu"]["sync_feedback_flow_rate"], 98.47);
        assert!(hare.status("printing")["mmu"]["encoder"].is_null());
    }

    #[test]
    fn the_boost_shows_only_while_printing() {
        let mut hare = homed(4);
        hare.apply(MmuCommand::FakeTangle { boosting: true }, IDLE).unwrap();

        let printing = hare.status("printing")["mmu"].clone();
        assert_eq!(printing["tangle_prevention"]["boosted"], true);
        assert_eq!(printing["sync_feedback_state"], "tension");
        assert_eq!(printing["sensors"]["filament_tension"], true);
        assert_eq!(hare.status("standby")["mmu"]["tangle_prevention"]["boosted"], false);
    }

    #[test]
    fn the_way_of_the_filament_follows_loads_and_can_be_held_midway() {
        let mut hare = homed(4);
        hare.apply(MmuCommand::Select { gate: 0 }, IDLE).unwrap();
        hare.apply(MmuCommand::Load, IDLE).unwrap();
        assert_eq!(
            (mmu(&hare)["filament_pos"].clone(), mmu(&hare)["filament_direction"].clone()),
            (json!(10), json!(1))
        );
        assert_eq!(mmu(&hare)["sensors"]["toolhead"], true);

        hare.apply(MmuCommand::FakeFilament { position: 3, direction: 1 }, IDLE).unwrap();
        let midway = mmu(&hare);
        assert_eq!(midway["filament_pos"], 3);
        assert!((midway["bowden_progress"].as_f64().unwrap() - 66.67).abs() < 0.1);
        assert_eq!(
            (midway["sensors"]["mmu_exit"].clone(), midway["sensors"]["extruder"].clone()),
            (json!(true), json!(false))
        );

        hare.apply(MmuCommand::Unload, IDLE).unwrap();
        assert_eq!(
            (mmu(&hare)["filament_pos"].clone(), mmu(&hare)["filament_direction"].clone()),
            (json!(0), json!(-1))
        );
    }
}
