//! Where each setting stands in the settings file: above the SAVE_CONFIG line what people set (on the screen or by
//! hand), below it what Sailhand keeps for itself (printer in use, last chosen steps, what it saw of each printer).

use super::{SavedPrinter, Settings, Steps, TemperaturePreset};
use crate::backlight::BacklightSettings;
use crate::klipper_ini::value::{self, ListLayout};
use crate::klipper_ini::{ConfigFile, Document, Section, Separator};
use crate::moonraker::Endpoint;
use crate::step_choices::StepChoices;
use std::str::FromStr;

const GENERAL: &str = "sailhand";
const PRINTER: &str = "printer";
const PRESET: &str = "preset";
const STEP_CHOICES: &str = "step_choices";
const BACKLIGHT: &str = "backlight";
/// Below the SAVE_CONFIG line only.
const SELECTED_STEPS: &str = "selected_steps";

mod key {
    pub(super) const FULLSCREEN: &str = "fullscreen";
    pub(super) const KEEP_SCREEN_ON: &str = "keep_screen_on";
    pub(super) const HIDE_TEMPERATURE_REPORTS: &str = "hide_temperature_reports";
    pub(super) const VERBOSE_LOG: &str = "verbose_log";
    pub(super) const ROTATION: &str = "rotation";
    pub(super) const MOONRAKER: &str = "moonraker";
    pub(super) const HIDDEN_MACROS: &str = "hidden_macros";
    pub(super) const EXTRUDER: &str = "extruder";
    pub(super) const BED: &str = "bed";
    pub(super) const MOVE_MM: &str = "move_mm";
    pub(super) const Z_OFFSET_MM: &str = "z_offset_mm";
    pub(super) const TEMPERATURE: &str = "temperature";
    pub(super) const EXTRUDE_MM: &str = "extrude_mm";
    pub(super) const EXTRUDE_SPEED: &str = "extrude_speed";
    pub(super) const DRIVE_SPEED: &str = "drive_speed";
    pub(super) const MANUAL_PROBE_MM: &str = "manual_probe_mm";
    pub(super) const DEVICE: &str = "device";
    pub(super) const BRIGHTNESS: &str = "brightness";
    pub(super) const OFF_WHEN_IDLE_MINUTES: &str = "off_when_idle_minutes";
    pub(super) const OFF_WHEN_PRINTING_MINUTES: &str = "off_when_printing_minutes";
    pub(super) const PRINTER: &str = "printer";
    pub(super) const UUID: &str = "uuid";
    pub(super) const KNOWN_MACROS: &str = "known_macros";
}

const GENERAL_KEYS: [&str; 5] =
    [key::FULLSCREEN, key::KEEP_SCREEN_ON, key::HIDE_TEMPERATURE_REPORTS, key::VERBOSE_LOG, key::ROTATION];
const ROTATIONS: [u16; 4] = [0, 90, 180, 270];
const PRINTER_KEYS: [&str; 2] = [key::MOONRAKER, key::HIDDEN_MACROS];
const PRESET_KEYS: [&str; 2] = [key::EXTRUDER, key::BED];
const STEP_CHOICE_KEYS: [&str; 6] =
    [key::MOVE_MM, key::Z_OFFSET_MM, key::TEMPERATURE, key::EXTRUDE_MM, key::EXTRUDE_SPEED, key::MANUAL_PROBE_MM];
const BACKLIGHT_KEYS: [&str; 4] =
    [key::DEVICE, key::BRIGHTNESS, key::OFF_WHEN_IDLE_MINUTES, key::OFF_WHEN_PRINTING_MINUTES];

/// Picks one general option out of the settings.
type FlagOf = fn(&Settings) -> bool;
/// Picks one list of step sizes.
type StepListOf = fn(&StepChoices) -> Option<&Vec<f64>>;

/// One option: its key and how its value is read and written.
struct Field<T> {
    key: &'static str,
    parse: fn(&str) -> Result<T, String>,
    format: fn(&T) -> String,
}

impl<T: PartialEq> Field<T> {
    /// The value in the section; one that cannot be read is an error naming section, key and line.
    fn read(&self, section: &Section) -> Result<Option<T>, String> {
        let Some(text) = section.get(self.key) else { return Ok(None) };
        (self.parse)(text)
            .map(Some)
            .map_err(|error| format!("{} {} (line {}): {error}", section.title(), self.key, section.line_of(self.key)))
    }

    /// Writes the value unless the file already says the same in its own spelling (`210.0` for `210`).
    fn write(&self, section: &mut Section, value: &T) {
        let current = section.get(self.key).and_then(|text| (self.parse)(text).ok());
        if current.as_ref() != Some(value) {
            section.set(self.key, &(self.format)(value));
        }
    }

    fn write_optional(&self, section: &mut Section, value: Option<&T>) {
        match value {
            Some(value) => self.write(section, value),
            None => {
                section.remove(self.key);
            }
        }
    }
}

fn flag(key: &'static str) -> Field<bool> {
    Field { key, parse: value::parse_bool, format: value::format_bool }
}

fn number<T: FromStr + ToString>(key: &'static str) -> Field<T> {
    Field { key, parse: value::parse_number::<T>, format: value::format_number::<T> }
}

/// The screen rotation in degrees: 0, 90, 180 or 270.
fn rotation(key: &'static str) -> Field<u16> {
    Field {
        key,
        parse: |text| {
            let degrees = value::parse_number::<u16>(text)?;
            if ROTATIONS.contains(&degrees) { Ok(degrees) } else { Err(format!("`{text}` is not 0, 90, 180 or 270")) }
        },
        format: value::format_number::<u16>,
    }
}

fn numbers(key: &'static str) -> Field<Vec<f64>> {
    Field { key, parse: value::parse_number_list::<f64>, format: |list| value::format_list(list, ListLayout::Inline) }
}

fn text(key: &'static str) -> Field<String> {
    Field { key, parse: value::parse_text, format: String::clone }
}

/// Macro names: one per line above the SAVE_CONFIG line, where people read them, in one line below it.
fn names(key: &'static str, layout: ListLayout) -> Field<Vec<String>> {
    let format: fn(&Vec<String>) -> String = match layout {
        ListLayout::Inline => |list| value::format_list(list, ListLayout::Inline),
        ListLayout::Lines => |list| value::format_list(list, ListLayout::Lines),
    };
    Field { key, parse: |text| Ok(value::parse_list(text)), format }
}

/// A Moonraker address, stored as [`Endpoint::display`] writes it; `voron.local:7125` reads as
/// `http://voron.local:7125`.
fn address(key: &'static str) -> Field<String> {
    Field {
        key,
        parse: |text| Endpoint::parse(text).map(|endpoint| endpoint.display()).map_err(|error| error.to_string()),
        format: String::clone,
    }
}

/// The settings in the file; a value that cannot be read is an error naming its line.
pub(super) fn read(file: &ConfigFile) -> Result<Settings, String> {
    let mut settings = Settings::default();
    let mut presets = Vec::new();
    for section in file.body.sections() {
        match (section.kind(), section.name()) {
            (GENERAL, None) => read_general(section, &mut settings)?,
            (PRINTER, Some(name)) => settings.printers.push(read_printer(section, name)?),
            (PRESET, Some(name)) => presets.push(read_preset(section, name)?),
            (STEP_CHOICES, None) => settings.step_choices = read_step_choices(section)?,
            (BACKLIGHT, None) => settings.backlight = read_backlight(section)?,
            (PRINTER | PRESET, None) => {
                return Err(format!(
                    "{} (line {}) needs a name, e.g. [{} voron]",
                    section.title(),
                    section.line(),
                    section.kind()
                ));
            }
            _ => log::warn!("settings: unknown section {} (line {}); it is ignored", section.title(), section.line()),
        }
    }
    // A new file holds the default presets; one without any (all commented out) means none.
    settings.temperature_presets = presets;
    read_saved(&file.saved, &mut settings);
    Ok(settings)
}

/// Writes into the file what changed between `before` (the settings as last read or written) and `after`.
/// Everything else above the SAVE_CONFIG line stays as it is: sections, values and comments made by hand, also
/// those added while Sailhand ran. The SAVE_CONFIG block is Sailhand's own and is written anew. Without `before`
/// (a new file) every general option and preset is written, so the file shows them.
pub(super) fn write(before: Option<&Settings>, after: &Settings, file: &mut ConfigFile) {
    let defaults = Settings::default();
    let base = before.unwrap_or(&defaults);
    write_general(before, after, &mut file.body);
    write_named(&mut file.body, PRINTER, &base.printers, &after.printers);
    let presets_before = before.map_or(&[][..], |before| before.temperature_presets.as_slice());
    write_named(&mut file.body, PRESET, presets_before, &after.temperature_presets);
    write_step_choices(&base.step_choices, &after.step_choices, &mut file.body);
    write_backlight(&base.backlight, &after.backlight, &mut file.body);
    file.saved = saved_block(after);
}

/// Writes the value when it changed; `None` removes the key.
fn write_if_changed<T: PartialEq>(section: &mut Section, field: Field<T>, before: Option<&T>, after: Option<&T>) {
    if before != after {
        field.write_optional(section, after);
    }
}

/// A list kept as one `[kind name]` section per item.
trait NamedSection: PartialEq {
    fn name(&self) -> &str;
    /// Whether nothing above the SAVE_CONFIG line differs.
    fn same_in_file(&self, other: &Self) -> bool {
        self == other
    }
    /// Writes the values that differ from `before`; all of them for an item that is new.
    fn write_changes(&self, before: Option<&Self>, section: &mut Section);
}

/// Removes the sections of items removed on the screen and writes the items that changed. A section of the same
/// kind that was never in `before` (made by hand) stays.
fn write_named<T: NamedSection>(body: &mut Document, kind: &str, before: &[T], after: &[T]) {
    for removed in before.iter().filter(|old| !after.iter().any(|new| new.name() == old.name())) {
        body.remove_section(kind, Some(removed.name()));
    }
    for item in after {
        let old = before.iter().find(|old| old.name() == item.name());
        if old.is_none_or(|old| !item.same_in_file(old)) {
            item.write_changes(old, body.ensure_section(kind, Some(item.name())));
        }
    }
}

fn read_general(section: &Section, settings: &mut Settings) -> Result<(), String> {
    warn_unknown(section, &GENERAL_KEYS);
    let defaults = Settings::default();
    settings.fullscreen = flag(key::FULLSCREEN).read(section)?.unwrap_or(defaults.fullscreen);
    settings.keep_screen_on = flag(key::KEEP_SCREEN_ON).read(section)?.unwrap_or(defaults.keep_screen_on);
    settings.hide_temperature_reports =
        flag(key::HIDE_TEMPERATURE_REPORTS).read(section)?.unwrap_or(defaults.hide_temperature_reports);
    settings.verbose_log = flag(key::VERBOSE_LOG).read(section)?.unwrap_or(defaults.verbose_log);
    settings.rotation = rotation(key::ROTATION).read(section)?.unwrap_or(defaults.rotation);
    Ok(())
}

fn write_general(before: Option<&Settings>, after: &Settings, body: &mut Document) {
    let options: [(&'static str, FlagOf); 4] = [
        (key::FULLSCREEN, |settings| settings.fullscreen),
        (key::KEEP_SCREEN_ON, |settings| settings.keep_screen_on),
        (key::HIDE_TEMPERATURE_REPORTS, |settings| settings.hide_temperature_reports),
        (key::VERBOSE_LOG, |settings| settings.verbose_log),
    ];
    let changed: Vec<_> =
        options.into_iter().filter(|(_, value)| before.is_none_or(|before| value(before) != value(after))).collect();
    // Only for the framebuffer fallback: written when set, never as a hint in a new file.
    let rotation_before = before.map_or(Settings::default().rotation, |before| before.rotation);
    if changed.is_empty() && rotation_before == after.rotation {
        return;
    }
    let section = body.ensure_section(GENERAL, None);
    for (key, value) in changed {
        flag(key).write(section, &value(after));
    }
    write_if_changed(section, rotation(key::ROTATION), Some(&rotation_before), Some(&after.rotation));
}

fn read_printer(section: &Section, name: &str) -> Result<SavedPrinter, String> {
    warn_unknown(section, &PRINTER_KEYS);
    let address = address(key::MOONRAKER)
        .read(section)?
        .ok_or_else(|| format!("{} (line {}) needs `moonraker: http://…`", section.title(), section.line()))?;
    let hidden_macros = names(key::HIDDEN_MACROS, ListLayout::Lines).read(section)?.unwrap_or_default();
    Ok(SavedPrinter { name: name.to_owned(), address, uuid: None, known_macros: Vec::new(), hidden_macros })
}

/// Only what stands above the SAVE_CONFIG line counts; uuid and known macros belong to Sailhand's block.
impl NamedSection for SavedPrinter {
    fn name(&self) -> &str {
        &self.name
    }

    fn same_in_file(&self, other: &Self) -> bool {
        self.address == other.address && self.hidden_macros == other.hidden_macros
    }

    fn write_changes(&self, before: Option<&Self>, section: &mut Section) {
        let hidden = |printer: &Self| (!printer.hidden_macros.is_empty()).then(|| printer.hidden_macros.clone());
        let address_before = before.map(|before| &before.address);
        write_if_changed(section, address(key::MOONRAKER), address_before, Some(&self.address));
        let hidden_before = before.and_then(hidden);
        let hidden_macros = names(key::HIDDEN_MACROS, ListLayout::Lines);
        write_if_changed(section, hidden_macros, hidden_before.as_ref(), hidden(self).as_ref());
    }
}

fn read_preset(section: &Section, name: &str) -> Result<TemperaturePreset, String> {
    warn_unknown(section, &PRESET_KEYS);
    let required = |key: &'static str| -> Result<f64, String> {
        number(key).read(section)?.ok_or_else(|| format!("{} needs `{key}: <°C>`", section.title()))
    };
    Ok(TemperaturePreset { name: name.to_owned(), extruder: required(key::EXTRUDER)?, bed: required(key::BED)? })
}

impl NamedSection for TemperaturePreset {
    fn name(&self) -> &str {
        &self.name
    }

    fn write_changes(&self, before: Option<&Self>, section: &mut Section) {
        write_if_changed(section, number(key::EXTRUDER), before.map(|before| &before.extruder), Some(&self.extruder));
        write_if_changed(section, number(key::BED), before.map(|before| &before.bed), Some(&self.bed));
    }
}

fn read_step_choices(section: &Section) -> Result<StepChoices, String> {
    warn_unknown(section, &STEP_CHOICE_KEYS);
    Ok(StepChoices {
        move_mm: numbers(key::MOVE_MM).read(section)?,
        z_offset_mm: numbers(key::Z_OFFSET_MM).read(section)?,
        temperature: numbers(key::TEMPERATURE).read(section)?,
        extrude_mm: numbers(key::EXTRUDE_MM).read(section)?,
        extrude_speed: numbers(key::EXTRUDE_SPEED).read(section)?,
        manual_probe_mm: numbers(key::MANUAL_PROBE_MM).read(section)?,
    })
}

/// The section only appears once something in it changed.
fn write_step_choices(before: &StepChoices, after: &StepChoices, body: &mut Document) {
    if before == after {
        return;
    }
    let section = body.ensure_section(STEP_CHOICES, None);
    let fields: [(&'static str, StepListOf); 6] = [
        (key::MOVE_MM, |choices| choices.move_mm.as_ref()),
        (key::Z_OFFSET_MM, |choices| choices.z_offset_mm.as_ref()),
        (key::TEMPERATURE, |choices| choices.temperature.as_ref()),
        (key::EXTRUDE_MM, |choices| choices.extrude_mm.as_ref()),
        (key::EXTRUDE_SPEED, |choices| choices.extrude_speed.as_ref()),
        (key::MANUAL_PROBE_MM, |choices| choices.manual_probe_mm.as_ref()),
    ];
    for (key, value) in fields {
        write_if_changed(section, numbers(key), value(before), value(after));
    }
}

fn read_backlight(section: &Section) -> Result<BacklightSettings, String> {
    warn_unknown(section, &BACKLIGHT_KEYS);
    Ok(BacklightSettings {
        device: text(key::DEVICE).read(section)?,
        brightness: number(key::BRIGHTNESS).read(section)?,
        off_when_idle_minutes: number(key::OFF_WHEN_IDLE_MINUTES).read(section)?,
        off_when_printing_minutes: number(key::OFF_WHEN_PRINTING_MINUTES).read(section)?,
    })
}

/// The section only appears once something in it changed.
fn write_backlight(before: &BacklightSettings, after: &BacklightSettings, body: &mut Document) {
    if before == after {
        return;
    }
    let section = body.ensure_section(BACKLIGHT, None);
    write_if_changed(section, text(key::DEVICE), before.device.as_ref(), after.device.as_ref());
    write_if_changed(section, number(key::BRIGHTNESS), before.brightness.as_ref(), after.brightness.as_ref());
    let (idle_before, idle_after) = (&before.off_when_idle_minutes, &after.off_when_idle_minutes);
    write_if_changed(section, number(key::OFF_WHEN_IDLE_MINUTES), idle_before.as_ref(), idle_after.as_ref());
    let (printing_before, printing_after) = (&before.off_when_printing_minutes, &after.off_when_printing_minutes);
    write_if_changed(
        section,
        number(key::OFF_WHEN_PRINTING_MINUTES),
        printing_before.as_ref(),
        printing_after.as_ref(),
    );
}

/// What Sailhand keeps for itself. A value there that cannot be read is only logged: the block is written anew.
fn read_saved(saved: &Document, settings: &mut Settings) {
    if let Some(section) = saved.section(GENERAL, None) {
        let by_name = lenient(text(key::PRINTER).read(section))
            .and_then(|name| settings.printers.iter().find(|printer| printer.name == name))
            .map(|printer| printer.address.clone());
        settings.moonraker = by_name.or_else(|| lenient(address(key::MOONRAKER).read(section)));
    }
    if let Some(section) = saved.section(SELECTED_STEPS, None) {
        let steps = &mut settings.steps;
        let read = |key: &'static str, current: f64| lenient(number(key).read(section)).unwrap_or(current);
        steps.move_mm = read(key::MOVE_MM, steps.move_mm);
        steps.z_offset_mm = read(key::Z_OFFSET_MM, steps.z_offset_mm);
        steps.temperature = read(key::TEMPERATURE, steps.temperature);
        steps.extrude_mm = read(key::EXTRUDE_MM, steps.extrude_mm);
        steps.extrude_speed = read(key::EXTRUDE_SPEED, steps.extrude_speed);
        steps.drive_speed = read(key::DRIVE_SPEED, steps.drive_speed);
    }
    for printer in &mut settings.printers {
        let Some(section) = saved.section(PRINTER, Some(&printer.name)) else { continue };
        printer.uuid = lenient(text(key::UUID).read(section));
        printer.known_macros = lenient(names(key::KNOWN_MACROS, ListLayout::Inline).read(section)).unwrap_or_default();
    }
}

fn saved_block(settings: &Settings) -> Document {
    let mut saved = Document::new("", Separator::Equals);
    if let Some(address_in_use) = &settings.moonraker {
        let section = saved.ensure_section(GENERAL, None);
        match settings.printer(address_in_use) {
            Some(printer) => text(key::PRINTER).write(section, &printer.name),
            None => address(key::MOONRAKER).write(section, address_in_use),
        }
    }
    write_selected_steps(&settings.steps, saved.ensure_section(SELECTED_STEPS, None));
    for printer in &settings.printers {
        if printer.uuid.is_none() && printer.known_macros.is_empty() {
            continue;
        }
        let section = saved.ensure_section(PRINTER, Some(&printer.name));
        text(key::UUID).write_optional(section, printer.uuid.as_ref());
        let known = (!printer.known_macros.is_empty()).then_some(&printer.known_macros);
        names(key::KNOWN_MACROS, ListLayout::Inline).write_optional(section, known);
    }
    saved
}

fn write_selected_steps(steps: &Steps, section: &mut Section) {
    number(key::MOVE_MM).write(section, &steps.move_mm);
    number(key::Z_OFFSET_MM).write(section, &steps.z_offset_mm);
    number(key::TEMPERATURE).write(section, &steps.temperature);
    number(key::EXTRUDE_MM).write(section, &steps.extrude_mm);
    number(key::EXTRUDE_SPEED).write(section, &steps.extrude_speed);
    number(key::DRIVE_SPEED).write(section, &steps.drive_speed);
}

fn lenient<T>(result: Result<Option<T>, String>) -> Option<T> {
    result.unwrap_or_else(|error| {
        log::warn!("settings: SAVE_CONFIG block, {error}; it is left out");
        None
    })
}

fn warn_unknown(section: &Section, known: &[&str]) {
    for (key, _, line) in section.entries().filter(|(key, _, _)| !known.contains(key)) {
        log::warn!("settings: {} has no option `{key}` (line {line}); it is ignored", section.title());
    }
}
