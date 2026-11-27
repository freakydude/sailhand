//! `--long-names`: the fake reports long but realistic names and texts, so UI snapshots show where text overflows.
//!
//! The simulation keeps its short names; this map renames them only on the wire. Everything the fake sends (replies,
//! notifications) goes through [`LongNames::to_long`], everything it receives (request parameters, file paths) through
//! [`LongNames::to_short`]. Every long name is unique and contains no other long name, so the way back is a plain
//! substring replacement.

use serde_json::{Map, Value};

/// Where a rename applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// A Klipper object or config section `<kind> <name>`; only the name changes.
    Object(&'static str),
    /// A string that is exactly the short name (a bare name like a power device, a key or a value).
    Exact,
    /// A string value under this key only (the short text is also used elsewhere with another meaning).
    Field(&'static str),
    /// Every occurrence inside any string (file names in paths, phrases inside console lines).
    Text,
}

#[derive(Debug, Clone, Copy)]
struct Rename {
    scope: Scope,
    short: &'static str,
    long: &'static str,
}

const fn rename(scope: Scope, short: &'static str, long: &'static str) -> Rename {
    Rename { scope, short, long }
}

/// Klipper does not limit the length of a section name (`configfile.py` takes the text between the brackets), so
/// object names get 40-60 characters, with words a user would pick to tell the parts apart.
const OBJECT_NAMES: [Rename; 10] = [
    rename(Scope::Object("heater_generic"), "chamber", "chamber_heater_ptc_400w_behind_the_left_side_panel"),
    rename(
        Scope::Object("temperature_sensor"),
        "raspberry_pi",
        "raspberry_pi_4b_host_in_the_electronics_bay_below_bed",
    ),
    rename(Scope::Object("fan_generic"), "nevermore", "nevermore_stealthmax_carbon_filter_in_the_rear_corner"),
    rename(Scope::Object("heater_fan"), "hotend", "stealthburner_hotend_fan_4010_on_the_toolhead_board"),
    rename(Scope::Object("controller_fan"), "board", "electronics_bay_fan_for_the_octopus_pro_and_drivers"),
    rename(Scope::Object("neopixel"), "chamber", "chamber_light_strip_along_the_front_top_extrusion"),
    rename(Scope::Object("led_effect"), "rainbow", "rainbow_chase_on_the_chamber_light_strip_while_idle"),
    rename(Scope::Object("gcode_macro"), "PARK", "PARK_TOOLHEAD_AT_THE_FRONT_CENTER_FOR_NOZZLE_CHECKS"),
    rename(Scope::Object("gcode_macro"), "PRINT_END", "PRINT_END_WITH_NOZZLE_WIPE_AND_CHAMBER_COOLDOWN_40C"),
    // Moonraker's `wled` component names a strip after its section `[wled <name>]`, also without a limit.
    rename(Scope::Exact, "enclosure", "enclosure_wled_strip_on_the_rear_top_extrusion_esp32"),
];

/// The bare names of the renamed macros, as `printer.gcode.help` lists them (Klipper upper-cases macro names).
const MACRO_NAMES: [Rename; 2] = [
    rename(Scope::Exact, "PARK", "PARK_TOOLHEAD_AT_THE_FRONT_CENTER_FOR_NOZZLE_CHECKS"),
    rename(Scope::Exact, "PRINT_END", "PRINT_END_WITH_NOZZLE_WIPE_AND_CHAMBER_COOLDOWN_40C"),
];

/// Moonraker's power devices are named after their section `[power <name>]`, without a length limit.
const POWER_DEVICES: [Rename; 1] =
    [rename(Scope::Exact, "light", "chamber_led_light_and_exhaust_relay_on_channel_two")];

/// File names of the demo files (`files.rs`): Linux allows 255 bytes per name (`NAME_MAX`), slicers build them from
/// their settings (PrusaSlicer/OrcaSlicer `output_filename_format`). `benchy.gcode` is the file the snapshots print,
/// so the current print and the history show the long name too; `long_job.gcode` stays short.
const FILE_NAMES: [Rename; 2] = [
    rename(
        Scope::Text,
        "benchy.gcode",
        "3DBenchy_the_jolly_3D_printing_torture_test_by_CreativeTools_se_scaled_150_percent_\
         0.4n_0.2mm_PLA_Signal_Red_Matte_VORON_TRIDENT_300_with_StealthBurner_and_CW2_extruder_\
         Arachne_walls_3_infill_15_gyroid_supports_tree_auto_brim_5mm_ironing_top_only.gcode",
    ),
    rename(
        Scope::Text,
        "calibration_cube.gcode",
        "Voron_Design_Cube_v7_ABS_0.4n_0.2mm_flow_and_dimension_check_45m.gcode",
    ),
];

/// Spoolman limits the vendor, filament name and material to 64 characters (assumption: `max_length=64` of the
/// fields in Spoolman's API models, not checked against its source here). Happy Hare and AFC copy them from the spool.
const SPOOL_TEXTS: [Rename; 4] = [
    rename(Scope::Exact, "Fake", "Shenzhen Example Additive Manufacturing Technology Co., Ltd."),
    rename(Scope::Exact, "Red PLA", "Signal Red PLA Basic Matte with Glitter Flakes, 1.75 mm, Refill"),
    rename(Scope::Exact, "Silk Red+Gold+Black", "Silk Tri-Colour Coextruded Red / Gold / Black Shimmer PLA+"),
    rename(Scope::Text, "PETG", "PETG-CF15 Carbon Fibre Reinforced Engineering Grade"),
];

/// Names that are free text in the profile of a filament changer: Happy Hare's `display_name` of a `[mmu_unit]`
/// (`config/base/mmu_hardware.cfg`), the unit and lane names of AFC (`[AFC_BoxTurtle <name>]`, `[AFC_stepper <name>]`).
/// `lane1` to `lane3` stay short, the snapshots look them up by name.
const CHANGER_NAMES: [Rename; 5] = [
    rename(Scope::Exact, "Unit 0", "Enraged Rabbit Carrot Feeder on the left shelf"),
    rename(Scope::Field("display_name"), "ERCF", "ERCF v2 with Filamentalist rewinders, upper shelf"),
    rename(Scope::Field("display_name"), "Tradrack", "Tradrack 1.0 next to the printer, eight gates"),
    rename(Scope::Text, "Turtle_1", "Turtle_1_BoxTurtle_on_top_of_the_enclosure"),
    rename(Scope::Text, "lane4", "lane4_spare_slot_for_abrasive_carbon_fibre"),
];

/// Texts the fake sends for the UI to show.
const MESSAGES: [Rename; 7] = [
    // A host name may have 63 characters per label (RFC 1123); Moonraker reports it as `hostname`, Happy Hare names it
    // in its Spoolman messages.
    rename(Scope::Text, "fake-printer", "voron-trident-300-workshop-basement-left-corner"),
    // Klipper appends `message_shutdown` of `klippy.py` to the reason of a shutdown.
    rename(
        Scope::Text,
        "Shutdown due to M112 command",
        "Shutdown due to M112 command\nOnce the underlying issue is corrected, use the\n\"FIRMWARE_RESTART\" command \
         to reset the firmware, reload the\nconfig, and restart the host software.\nPrinter is shutdown",
    ),
    // The error of a print: instead of the short move error, Klipper's error of `extruder.py` `check_move`.
    rename(
        Scope::Text,
        "Move out of range: 310.000 150.000 12.400 [482.120]",
        "Move exceeds maximum extrusion (2.345mm^2 vs 0.640mm^2)\nSee the 'max_extrude_cross_section' config option \
         for details",
    ),
    // `action:prompt_*` texts of the demo prompts (`gcode/respond.rs`): Klipper passes them on as they are, without a
    // limit (`docs/G-Codes.md`, `RESPOND`).
    rename(
        Scope::Text,
        "Is the bed empty?",
        "Is the build plate completely empty, including purge lines and skirt leftovers of the last print?",
    ),
    rename(
        Scope::Text,
        "Anything on it gets crushed against the head.",
        "Anything on it gets crushed against the head, which can bend the plate, break the probe or tear off the \
         nozzle wiper.",
    ),
    rename(Scope::Text, "prompt_begin Filament runout", "prompt_begin Filament runout detected by the toolhead sensor"),
    rename(Scope::Text, "Abort print|", "Abort the print and keep the bed heated for part removal|"),
];

const ALL_GROUPS: [&[Rename]; 7] =
    [&OBJECT_NAMES, &MACRO_NAMES, &POWER_DEVICES, &FILE_NAMES, &SPOOL_TEXTS, &CHANGER_NAMES, &MESSAGES];

/// The renames on the wire; empty without `--long-names`.
#[derive(Debug, Clone, Default)]
pub struct LongNames {
    renames: Vec<Rename>,
}

impl LongNames {
    pub fn new(enabled: bool) -> Self {
        let renames =
            if enabled { ALL_GROUPS.iter().flat_map(|group| group.iter().copied()).collect() } else { Vec::new() };
        Self { renames }
    }

    /// What the fake sends: short names become long.
    pub fn to_long(&self, value: Value) -> Value {
        if self.renames.is_empty() { value } else { self.long_value(value, None) }
    }

    /// What the fake receives: long names become short again.
    pub fn to_short(&self, value: Value) -> Value {
        if self.renames.is_empty() {
            return value;
        }
        map_strings(value, &|text| self.short_text(text))
    }

    /// A file path of an HTTP request.
    pub fn path_to_short(&self, path: &str) -> String {
        self.short_text(path)
    }

    fn long_value(&self, value: Value, key: Option<&str>) -> Value {
        match value {
            Value::String(text) => Value::String(self.long_text(&text, key)),
            Value::Array(items) => Value::Array(items.into_iter().map(|item| self.long_value(item, key)).collect()),
            Value::Object(entries) => Value::Object(
                entries
                    .into_iter()
                    .map(|(name, item)| {
                        let item = self.long_value(item, Some(&name));
                        (self.long_text(&name, None), item)
                    })
                    .collect::<Map<_, _>>(),
            ),
            other => other,
        }
    }

    fn long_text(&self, text: &str, key: Option<&str>) -> String {
        self.renames.iter().find_map(|rename| rename.whole(text, key)).unwrap_or_else(|| {
            self.renames
                .iter()
                .filter(|rename| rename.scope == Scope::Text)
                .fold(text.to_owned(), |text, rename| text.replace(rename.short, rename.long))
        })
    }

    fn short_text(&self, text: &str) -> String {
        self.renames.iter().fold(text.to_owned(), |text, rename| text.replace(rename.long, rename.short))
    }
}

impl Rename {
    /// The long form if the whole string is this name.
    fn whole(&self, text: &str, key: Option<&str>) -> Option<String> {
        match self.scope {
            Scope::Object(kind) => {
                let name = text.strip_prefix(kind)?.strip_prefix(' ')?;
                (name == self.short).then(|| format!("{kind} {}", self.long))
            }
            Scope::Exact => (text == self.short).then(|| self.long.to_owned()),
            Scope::Field(field) => (key == Some(field) && text == self.short).then(|| self.long.to_owned()),
            Scope::Text => None,
        }
    }
}

fn map_strings(value: Value, change: &impl Fn(&str) -> String) -> Value {
    match value {
        Value::String(text) => Value::String(change(&text)),
        Value::Array(items) => Value::Array(items.into_iter().map(|item| map_strings(item, change)).collect()),
        Value::Object(entries) => {
            Value::Object(entries.into_iter().map(|(name, item)| (change(&name), map_strings(item, change))).collect())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn all() -> Vec<Rename> {
        LongNames::new(true).renames
    }

    #[test]
    fn without_the_option_nothing_changes() {
        let names = LongNames::new(false);
        let status =
            json!({ "fan_generic nevermore": { "speed": 0.5 }, "print_stats": { "filename": "benchy.gcode" } });
        assert_eq!(names.to_long(status.clone()), status);
    }

    #[test]
    fn the_lengths_follow_their_limits() {
        for rename in OBJECT_NAMES.iter().chain(&MACRO_NAMES).chain(&POWER_DEVICES) {
            assert!((40..=60).contains(&rename.long.len()), "{} has {} characters", rename.long, rename.long.len());
        }
        let [benchy, cube] = FILE_NAMES;
        assert!((240..=255).contains(&benchy.long.len()), "NAME_MAX: {}", benchy.long.len());
        assert!((70..=90).contains(&cube.long.len()), "{}", cube.long.len());
        assert!(FILE_NAMES.iter().all(|file| !file.long.contains(' ') && file.long.ends_with(".gcode")));
        assert!(SPOOL_TEXTS.iter().all(|text| text.long.len() <= 64));
        assert!(MESSAGES[0].long.len() <= 63 && MESSAGES[0].long.len() >= 40);
    }

    #[test]
    fn the_way_back_is_unique_and_texts_do_not_rename_each_other() {
        let renames = all();
        for (index, rename) in renames.iter().enumerate() {
            for (other_index, other) in renames.iter().enumerate() {
                if other.long == rename.long {
                    continue;
                }
                assert!(!rename.long.contains(other.long), "{} contains {}", rename.long, other.long);
                if other.scope == Scope::Text && other_index != index {
                    assert!(!rename.long.contains(other.short), "{} contains {}", rename.long, other.short);
                }
            }
        }
    }

    #[test]
    fn objects_keys_fields_and_texts_get_long_and_come_back_short() {
        let names = LongNames::new(true);
        let sent = names.to_long(json!({
            "fan_generic nevermore": { "speed": 0.5 },
            "neopixel chamber": {},
            "heater_generic chamber": {},
            "objects": ["gcode_macro PARK", "temperature_sensor mcu"],
            "help": { "PARK": "G-Code macro" },
            "unit": { "display_name": "ERCF", "vendor": "ERCF" },
            "print_stats": { "filename": "benchy.gcode", "message": "chamber" },
        }));
        assert!(sent.get("fan_generic nevermore_stealthmax_carbon_filter_in_the_rear_corner").is_some());
        assert!(sent.get("neopixel chamber_light_strip_along_the_front_top_extrusion").is_some());
        assert!(sent.get("heater_generic chamber_heater_ptc_400w_behind_the_left_side_panel").is_some());
        assert_eq!(sent["objects"][1], "temperature_sensor mcu");
        assert!(sent["help"].get("PARK_TOOLHEAD_AT_THE_FRONT_CENTER_FOR_NOZZLE_CHECKS").is_some());
        assert_eq!(sent["unit"]["vendor"], "ERCF");
        assert_ne!(sent["unit"]["display_name"], "ERCF");
        assert_eq!(sent["print_stats"]["message"], "chamber");
        assert!(sent["print_stats"]["filename"].as_str().unwrap().starts_with("3DBenchy_"));

        let script = format!("SET_FAN_SPEED FAN={} SPEED=1\n{}", OBJECT_NAMES[2].long, MACRO_NAMES[0].long);
        assert_eq!(
            names.to_short(json!({ "script": script })),
            json!({ "script": "SET_FAN_SPEED FAN=nevermore SPEED=1\nPARK" })
        );
        assert_eq!(names.path_to_short(FILE_NAMES[0].long), "benchy.gcode");
    }
}
