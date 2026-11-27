//! Connects `UiSettings` with the settings file and applies changes immediately.

use crate::actions::slint_decimal;
use crate::items::float_model;
use crate::network_controller::NetworkController;
use crate::presenter::{DisplayChoices, Presenter};
use crate::{AppWindow, Backlight, ChoiceItem, PresetItem, UiSettings};
use sailhand_core::Snapshot;
use sailhand_core::backlight::{BacklightService, BacklightSettings, usable_brightness};
use sailhand_core::domain::{chamber, light};
use sailhand_core::settings::{MAX_SHOWN_PRESETS, SavedPrinter, Settings, SettingsFile, TemperaturePreset};
use sailhand_core::sources::PrinterStatus;
use sailhand_core::sources::klipper::moves::DRIVE_INTERVAL;
use sailhand_core::step_choices::{StepLists, mirrored_row, nearest_step};
use sailhand_core::{Endpoint, PrinterService};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

/// What the settings act on.
pub struct Bridges {
    pub service: PrinterService,
    pub presenter: Rc<Presenter>,
    pub network: Rc<NetworkController>,
    pub backlight: BacklightService,
}

pub struct SettingsController {
    window: slint::Weak<AppWindow>,
    file: RefCell<SettingsFile>,
    /// File contents; an address from the command line is not saved.
    settings: RefCell<Settings>,
    /// The address in use (`Endpoint::display`), also when it came from the command line.
    active: RefCell<String>,
    service: PrinterService,
    presenter: Rc<Presenter>,
    network: Rc<NetworkController>,
    backlight: BacklightService,
    presets: Rc<VecModel<PresetItem>>,
}

impl SettingsController {
    pub fn bind(
        window: &AppWindow,
        file: SettingsFile,
        settings: Settings,
        endpoint: &Endpoint,
        bridges: Bridges,
    ) -> Rc<Self> {
        let Bridges { service, presenter, network, backlight } = bridges;
        let controller = Rc::new(Self {
            window: window.as_weak(),
            file: RefCell::new(file),
            settings: RefCell::new(settings),
            active: RefCell::new(endpoint.display()),
            service,
            presenter,
            network,
            backlight,
            presets: Rc::default(),
        });
        controller.apply_hidden_macros();
        controller.apply_display_choices();
        let weak = Rc::downgrade(&controller);
        controller.presenter.on_printer_ready(move |snapshot| {
            if let Some(controller) = weak.upgrade() {
                controller.note_found_choices(snapshot);
            }
        });
        controller.presenter.set_hide_temperature_reports(controller.settings.borrow().hide_temperature_reports);
        let weak = Rc::downgrade(&controller);
        controller.presenter.on_macros_loaded(move |macros| {
            if let Some(controller) = weak.upgrade() {
                controller.reconcile_macros(macros);
            }
        });
        controller.show(window, endpoint);
        controller.connect_callbacks(window);
        controller
    }

    pub fn preset(&self, index: usize) -> Option<TemperaturePreset> {
        self.settings.borrow().temperature_presets.get(index).cloned()
    }

    pub fn fullscreen(&self) -> bool {
        self.settings.borrow().fullscreen
    }

    pub fn keep_screen_on(&self) -> bool {
        self.settings.borrow().keep_screen_on
    }

    fn show(&self, window: &AppWindow, endpoint: &Endpoint) {
        let ui = window.global::<UiSettings>();
        let settings = self.settings.borrow();
        ui.set_moonraker(endpoint.display().into());
        ui.set_fullscreen(settings.fullscreen);
        ui.set_fullscreen_available(cfg!(not(target_os = "android")));
        ui.set_keep_screen_on_available(cfg!(target_os = "android"));
        ui.set_keep_screen_on(settings.keep_screen_on);
        ui.set_log_export_available(cfg!(target_os = "android"));
        ui.set_hide_temperature_reports(settings.hide_temperature_reports);
        ui.set_move_step(settings.steps.move_mm as f32);
        ui.set_drive_speed(settings.steps.drive_speed as f32);
        ui.set_drive_interval(DRIVE_INTERVAL.as_millis() as i64);
        ui.set_z_offset_step(settings.steps.z_offset_mm as f32);
        ui.set_temperature_step(settings.steps.temperature as f32);
        ui.set_extrude_length(settings.steps.extrude_mm as f32);
        ui.set_extrude_speed(settings.steps.extrude_speed as f32);
        let lists = settings.step_choices.lists();
        show_step_lists(&ui, &lists);
        self.presenter.set_extrude_choices(lists.extrude_mm);
        if cfg!(not(target_os = "android")) {
            ui.set_settings_file(self.file.borrow().path().display().to_string().into());
        }
        ui.set_version(sailhand_core::version_label().into());
        let backlight = window.global::<Backlight>();
        backlight.set_idle_minutes(minutes_shown(settings.backlight.off_when_idle_minutes));
        backlight.set_printing_minutes(minutes_shown(settings.backlight.off_when_printing_minutes));
        let presets = settings.temperature_presets.iter().take(MAX_SHOWN_PRESETS).map(preset_item);
        self.presets.set_vec(presets.collect::<Vec<_>>());
        ui.set_presets(ModelRc::from(self.presets.clone()));
    }

    fn connect_callbacks(self: &Rc<Self>, window: &AppWindow) {
        let ui = window.global::<UiSettings>();
        let controller = Rc::downgrade(self);
        ui.on_choose_light(move |object| {
            if let Some(controller) = controller.upgrade() {
                controller.choose(|printer| printer.light = Some(object.to_string()));
            }
        });
        let controller = Rc::downgrade(self);
        ui.on_choose_chamber(move |object| {
            if let Some(controller) = controller.upgrade() {
                controller.choose(|printer| printer.chamber = Some(object.to_string()));
            }
        });
        let controller = Rc::downgrade(self);
        ui.on_steps_changed(move || {
            if let Some(controller) = controller.upgrade() {
                controller.store_steps();
            }
        });
        let controller = Rc::downgrade(self);
        ui.on_fullscreen_toggled(move |fullscreen| {
            if let Some(controller) = controller.upgrade() {
                controller.change_fullscreen(fullscreen);
            }
        });
        let controller = Rc::downgrade(self);
        ui.on_keep_screen_on_toggled(move |on| {
            if let Some(controller) = controller.upgrade() {
                #[cfg(target_os = "android")]
                crate::android::keep_screen_on(on);
                controller.update(|settings| settings.keep_screen_on = on);
            }
        });
        #[cfg(target_os = "android")]
        {
            let window = window.as_weak();
            ui.on_log_export_clicked(move || export_log(&window));
        }
        let controller = Rc::downgrade(self);
        ui.on_hide_temperature_reports_toggled(move |hide| {
            if let Some(controller) = controller.upgrade() {
                controller.update(|settings| settings.hide_temperature_reports = hide);
                controller.presenter.set_hide_temperature_reports(hide);
            }
        });
        let backlight = window.global::<Backlight>();
        let controller = Rc::downgrade(self);
        backlight.on_brightness_chosen(move |percent| {
            if let Some(controller) = controller.upgrade() {
                controller.change_brightness(percent);
            }
        });
        let controller = Rc::downgrade(self);
        backlight.on_idle_minutes_chosen(move |minutes| {
            if let Some(controller) = controller.upgrade() {
                controller.change_timeouts(|settings| settings.off_when_idle_minutes = minutes_setting(minutes));
            }
        });
        let controller = Rc::downgrade(self);
        backlight.on_printing_minutes_chosen(move |minutes| {
            if let Some(controller) = controller.upgrade() {
                controller.change_timeouts(|settings| settings.off_when_printing_minutes = minutes_setting(minutes));
            }
        });
    }

    /// Applies the brightness and keeps it for the next start.
    fn change_brightness(&self, percent: f32) {
        let percent = usable_brightness(percent.round().clamp(0.0, 100.0) as u8);
        self.backlight.set_brightness(percent);
        self.update(|settings| settings.backlight.brightness = Some(percent));
    }

    fn change_timeouts(&self, change: impl FnOnce(&mut BacklightSettings)) {
        self.update(|settings| change(&mut settings.backlight));
        self.backlight.set_timeouts(self.settings.borrow().backlight.timeouts());
    }

    fn store_steps(&self) {
        let Some(window) = self.window.upgrade() else { return };
        let ui = window.global::<UiSettings>();
        self.update(|settings| {
            settings.steps.move_mm = slint_decimal(ui.get_move_step());
            settings.steps.z_offset_mm = slint_decimal(ui.get_z_offset_step());
            settings.steps.temperature = slint_decimal(ui.get_temperature_step());
            settings.steps.extrude_mm = slint_decimal(ui.get_extrude_length());
            settings.steps.extrude_speed = slint_decimal(ui.get_extrude_speed());
            settings.steps.drive_speed = slint_decimal(ui.get_drive_speed());
        });
    }

    /// The saved printers, as in the settings file.
    pub fn saved_printers(&self) -> Vec<SavedPrinter> {
        self.settings.borrow().printers.clone()
    }

    pub fn active_address(&self) -> String {
        self.active.borrow().clone()
    }

    /// Uses this printer from now on, also after the next start, and keeps it.
    pub fn switch_printer(&self, endpoint: Endpoint, name: &str, uuid: Option<&str>) {
        let display = endpoint.display();
        self.update(|settings| settings.choose_printer(&endpoint, name, uuid));
        self.active.replace(display.clone());
        if let Some(window) = self.window.upgrade() {
            let ui = window.global::<UiSettings>();
            ui.set_moonraker(display.as_str().into());
            // The new printer offers its own once it is ready.
            ui.set_status_bar_choices_available(false);
        }
        self.apply_hidden_macros();
        self.apply_display_choices();
        self.network.set_endpoint(&endpoint);
        self.service.connect_to(endpoint);
        self.presenter.show_notice(&format!("Connecting to {name}"), false);
    }

    /// Removes the saved printer; the display keeps using it if it is the active one. Returns whether it was saved.
    pub fn forget_printer(&self, address: &str) -> bool {
        let mut forgotten = false;
        self.update(|settings| forgotten = settings.forget_printer(address));
        forgotten
    }

    fn change_fullscreen(&self, fullscreen: bool) {
        if let Some(window) = self.window.upgrade() {
            window.window().set_fullscreen(fullscreen);
        }
        self.update(|settings| settings.fullscreen = fullscreen);
    }

    /// Shows a macro in the console or hides it; the choice is kept in the settings file.
    pub fn set_macro_visible(&self, name: &str, visible: bool) {
        let active = self.active_address();
        self.update(|settings| {
            let Some(printer) = settings.printer_mut(&active) else { return };
            printer.hidden_macros.retain(|hidden| hidden != name);
            if !visible {
                printer.hidden_macros.push(name.to_owned());
            }
        });
        self.apply_hidden_macros();
    }

    /// Forgets macros the printer no longer has; new macros are known and visible.
    fn reconcile_macros(&self, macros: &[String]) {
        let active = self.active_address();
        self.update(|settings| {
            if let Some(printer) = settings.printer_mut(&active) {
                printer.reconcile_macros(macros);
            }
        });
        self.apply_hidden_macros();
    }

    /// Writes the light and chamber found on the printer in use into its entries, where they are still missing (also
    /// as none), so they are found only once; from then on only the entries count. A printer that is not saved (an
    /// address from the command line) keeps being searched by name. Also offers what the printer has for a choice.
    fn note_found_choices(&self, snapshot: &Snapshot) {
        let active = self.active_address();
        if snapshot.endpoint != active {
            return;
        }
        self.show_choices(snapshot);
        let missing = self
            .settings
            .borrow()
            .printer(&active)
            .is_some_and(|printer| printer.light.is_none() || printer.chamber.is_none());
        if !missing {
            return;
        }
        let (found_light, found_chamber) = (light::detect(&snapshot.sources), chamber::detect(&snapshot.sources));
        self.update(|settings| {
            if let Some(printer) = settings.printer_mut(&active) {
                printer.fill_missing_choices(found_light, found_chamber);
            }
        });
        self.apply_display_choices();
    }

    /// The lights and the chamber candidates of a saved printer, for the settings.
    fn show_choices(&self, snapshot: &Snapshot) {
        let Some(window) = self.window.upgrade() else { return };
        let ui = window.global::<UiSettings>();
        let saved = self.settings.borrow().printer(&self.active.borrow()).is_some();
        ui.set_status_bar_choices_available(saved);
        let status = &snapshot.status;
        let lights = light::lights(&snapshot.sources).map(|object| choice_item(object, light_label(status, object)));
        let chambers =
            chamber::candidates(&snapshot.sources).map(|object| choice_item(object, temperature_label(status, object)));
        set_choices(&ui.get_light_choices(), lights.collect(), |rows| ui.set_light_choices(rows));
        set_choices(&ui.get_chamber_choices(), chambers.collect(), |rows| ui.set_chamber_choices(rows));
    }

    /// A choice made in the settings for the printer in use; it shows at once.
    fn choose(&self, change: impl FnOnce(&mut SavedPrinter)) {
        let active = self.active_address();
        self.update(|settings| {
            if let Some(printer) = settings.printer_mut(&active) {
                change(printer);
            }
        });
        if self.apply_display_choices() {
            self.presenter.show_again();
        }
    }

    /// Hands the choices of the printer in use to the presenter and the settings; returns whether they changed.
    fn apply_display_choices(&self) -> bool {
        let choices = self
            .settings
            .borrow()
            .printer(&self.active.borrow())
            .map(|printer| DisplayChoices { light: printer.light.clone(), chamber: printer.chamber.clone() })
            .unwrap_or_default();
        if let Some(window) = self.window.upgrade() {
            let ui = window.global::<UiSettings>();
            ui.set_light_choice(choices.light.clone().unwrap_or_default().into());
            ui.set_chamber_choice(choices.chamber.clone().unwrap_or_default().into());
        }
        self.presenter.set_display_choices(choices)
    }

    fn apply_hidden_macros(&self) {
        let hidden = self.settings.borrow().hidden_macros(&self.active.borrow());
        self.presenter.set_hidden_macros(hidden);
    }

    fn update(&self, change: impl FnOnce(&mut Settings)) {
        let mut settings = self.settings.borrow_mut();
        change(&mut settings);
        if let Err(error) = self.file.borrow_mut().save(&settings) {
            log::warn!("{error}");
            self.presenter.show_notice(&error.to_string(), true);
        }
    }
}

/// The standby time of the settings file as the buttons show it: minutes, 0 for never.
fn choice_item(object: &str, label: &str) -> ChoiceItem {
    ChoiceItem { object: object.into(), label: label.into() }
}

/// The readable name of a light as the status reports it; the object until it reports.
fn light_label<'a>(status: &'a PrinterStatus, object: &'a str) -> &'a str {
    status.leds.iter().find(|led| led.object == object).map_or(object, |led| led.label.as_str())
}

/// The readable name of a heater or sensor; the object until it reports.
fn temperature_label<'a>(status: &'a PrinterStatus, object: &'a str) -> &'a str {
    let sensor = || status.sensors.iter().find(|sensor| sensor.object == object).map(|sensor| sensor.label.as_str());
    status.heater(object).map(|heater| heater.label.as_str()).or_else(sensor).unwrap_or(object)
}

/// Replaces the rows only when they differ, so an open list keeps its scroll position.
fn set_choices(current: &ModelRc<ChoiceItem>, rows: Vec<ChoiceItem>, set: impl FnOnce(ModelRc<ChoiceItem>)) {
    if current.iter().ne(rows.iter().cloned()) {
        set(ModelRc::new(VecModel::from(rows)));
    }
}

fn minutes_shown(minutes: Option<u32>) -> i32 {
    minutes.map_or(0, |minutes| minutes as i32)
}

fn minutes_setting(minutes: i32) -> Option<u32> {
    u32::try_from(minutes).ok().filter(|minutes| *minutes > 0)
}

fn preset_item(preset: &TemperaturePreset) -> PresetItem {
    PresetItem { name: preset.name.as_str().into(), extruder: preset.extruder as f32, bed: preset.bed as f32 }
}

/// Copies the log into the Download folder and shows where it went (Android).
#[cfg(target_os = "android")]
fn export_log(window: &slint::Weak<AppWindow>) {
    let Some(strong) = window.upgrade() else { return };
    let ui = strong.global::<UiSettings>();
    ui.set_log_export_running(true);
    ui.set_log_export_result("".into());
    let window = window.clone();
    crate::android::log_export::start(move |result| {
        // Nothing to do when the window is gone by then.
        let _ = slint::invoke_from_event_loop(move || {
            let Some(window) = window.upgrade() else { return };
            let ui = window.global::<UiSettings>();
            ui.set_log_export_running(false);
            let text = match result {
                Ok(path) => slint::format!("Saved to {path}"),
                Err(reason) => slint::format!("Export failed: {reason}"),
            };
            ui.set_log_export_result(text);
        });
    });
}

/// Offers the configured step sizes; a remembered selection that is not among them becomes the nearest one.
fn show_step_lists(ui: &UiSettings<'_>, lists: &StepLists) {
    ui.set_move_steps(float_model(&lists.move_mm));
    ui.set_z_offset_steps(float_model(&lists.z_offset_mm));
    ui.set_temperature_steps(float_model(&lists.temperature));
    ui.set_extrude_speeds(float_model(&lists.extrude_speed));
    ui.set_manual_probe_steps(float_model(&mirrored_row(&lists.manual_probe_mm)));
    ui.set_move_step(nearest_step(&lists.move_mm, slint_decimal(ui.get_move_step())) as f32);
    ui.set_z_offset_step(nearest_step(&lists.z_offset_mm, slint_decimal(ui.get_z_offset_step())) as f32);
    ui.set_temperature_step(nearest_step(&lists.temperature, slint_decimal(ui.get_temperature_step())) as f32);
    ui.set_extrude_speed(nearest_step(&lists.extrude_speed, slint_decimal(ui.get_extrude_speed())) as f32);
    let drive_speeds: Vec<f64> = ui.get_drive_speeds().iter().map(slint_decimal).collect();
    ui.set_drive_speed(nearest_step(&drive_speeds, slint_decimal(ui.get_drive_speed())) as f32);
}
