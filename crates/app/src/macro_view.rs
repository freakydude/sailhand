//! The macros the printer offers, which of them are shown, the form for the parameters of one, and the last run of
//! the chosen one with the answers during it.

use crate::console_view::console_items;
use crate::format;
use crate::items::replace_rows;
use crate::{ConsoleItem, MacroItem, MacroParamItem, MacroRun as MacroRunItem, MacroRunState, Printer};
use sailhand_core::domain::macro_runs::{MacroRun, MacroRuns, RunOutcome, macro_name};
use sailhand_core::sources::klipper::{MacroDetails, MacroForm, offered_macros};
use sailhand_core::sources::moonraker_server::gcode_store::ConsoleLine;
use slint::{Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

type MacrosLoaded = Box<dyn Fn(&[String])>;

#[derive(Default)]
pub struct MacroView {
    /// The names of the visible macros.
    visible: Rc<VecModel<SharedString>>,
    /// All macros with their visibility, for the settings.
    items: Rc<VecModel<MacroItem>>,
    form: Rc<VecModel<MacroParamItem>>,
    form_state: RefCell<MacroForm>,
    all: RefCell<Vec<String>>,
    hidden: RefCell<Vec<String>>,
    details: RefCell<Arc<BTreeMap<String, MacroDetails>>>,
    loaded: RefCell<Option<MacrosLoaded>>,
    runs: RefCell<MacroRuns>,
    /// The macro chosen on the Macros tab; its last run is shown.
    selected: RefCell<String>,
    run_lines: Rc<VecModel<ConsoleItem>>,
}

impl MacroView {
    pub fn bind(&self, printer: &Printer) {
        printer.set_macros(ModelRc::from(self.visible.clone()));
        printer.set_macro_items(ModelRc::from(self.items.clone()));
        printer.set_macro_form(ModelRc::from(self.form.clone()));
        printer.set_macro_run_lines(ModelRc::from(self.run_lines.clone()));
    }

    /// A macro was chosen: its last run is shown.
    pub fn select(&self, printer: &Printer, name: &str) {
        *self.selected.borrow_mut() = name.to_owned();
        printer.set_macro_selected(SharedString::from(name));
        self.show_run(printer);
    }

    /// Whether the chosen macro is this one (the macro a script calls).
    pub fn is_selected(&self, script: &str) -> bool {
        let selected = self.selected.borrow();
        !selected.is_empty() && macro_name(script).eq_ignore_ascii_case(&selected)
    }

    /// A macro was sent from here.
    pub fn run_started(&self, printer: &Printer, id: u64, script: &str, now: f64) {
        self.runs.borrow_mut().start(id, script, now);
        if self.is_selected(script) {
            self.show_run(printer);
        }
    }

    /// A line arrived live; true if it went to the run of the chosen macro, whose card shows it.
    pub fn add_line(&self, line: &ConsoleLine) -> bool {
        let reached = self.runs.borrow_mut().add_line(line).into_iter().any(|name| self.is_selected(name));
        if reached {
            for item in console_items(line) {
                self.run_lines.push(item);
            }
        }
        reached
    }

    /// The answer of a run came; `error` is why it failed.
    pub fn run_ended(&self, printer: &Printer, id: u64, error: Option<String>, now: f64) {
        let ended = self.runs.borrow_mut().finish(id, error, now).is_some_and(|name| self.is_selected(name));
        if ended {
            self.show_run(printer);
        }
    }

    /// Another printer: its macros and runs are others.
    pub fn forget_runs(&self, printer: &Printer) {
        self.runs.borrow_mut().clear();
        self.select(printer, "");
    }

    fn show_run(&self, printer: &Printer) {
        let runs = self.runs.borrow();
        let run = runs.last(&self.selected.borrow());
        printer.set_macro_run(run.map(run_item).unwrap_or_default());
        let lines: Vec<ConsoleItem> =
            run.map(|run| run.lines.iter().flat_map(console_items).collect()).unwrap_or_default();
        self.run_lines.set_vec(lines);
    }

    /// Called with the printer's macros after a connection, so the settings can forget vanished ones.
    pub fn on_loaded(&self, handler: impl Fn(&[String]) + 'static) {
        *self.loaded.borrow_mut() = Some(Box::new(handler));
    }

    /// Takes over the printer's macros and their details. An empty list while disconnected must not count as "all
    /// macros vanished".
    pub fn show(&self, macros: &[String], details: &Arc<BTreeMap<String, MacroDetails>>, connected: bool) {
        self.set_details(details);
        if self.all.borrow().as_slice() == macros {
            return;
        }
        *self.all.borrow_mut() = macros.to_vec();
        self.refresh();
        if connected
            && !macros.is_empty()
            && let Some(handler) = self.loaded.borrow().as_ref()
        {
            handler(macros);
        }
    }

    /// Applies the hidden macros from the settings.
    pub fn set_hidden(&self, hidden: Vec<String>) {
        *self.hidden.borrow_mut() = hidden;
        self.refresh();
    }

    /// Whether the macro takes parameters, so the form has to come before it runs.
    pub fn has_params(&self, name: &str) -> bool {
        self.details.borrow().get(&name.to_ascii_uppercase()).is_some_and(|details| !details.params.is_empty())
    }

    /// Fills the form with the parameters of a macro; empty values are left to Klipper's defaults.
    pub fn open_form(&self, printer: &Printer, name: &str) {
        let form = MacroForm::open(name, self.details.borrow().get(&name.to_ascii_uppercase()));
        let rows: Vec<MacroParamItem> = form
            .fields()
            .iter()
            .map(|field| MacroParamItem {
                name: SharedString::from(field.name.as_str()),
                placeholder: SharedString::from(field.placeholder.as_str()),
                value: SharedString::from(field.value.as_str()),
                numeric: field.numeric,
            })
            .collect();
        *self.form_state.borrow_mut() = form;
        self.form.set_vec(rows);
        printer.set_macro_form_name(SharedString::from(self.form_state.borrow().name()));
    }

    /// Sets one parameter of the open form; the error says why the value is refused.
    pub fn set_param(&self, index: usize, value: &str) -> Result<(), String> {
        self.form_state.borrow_mut().set_value(index, value).map_err(|error| error.to_string())?;
        if let Some(mut row) = self.form.row_data(index) {
            row.value = SharedString::from(value);
            self.form.set_row_data(index, row);
        }
        Ok(())
    }

    /// The command for the open form, e.g. `PARK Z=20`.
    pub fn form_script(&self) -> String {
        self.form_state.borrow().script()
    }

    fn set_details(&self, details: &Arc<BTreeMap<String, MacroDetails>>) {
        if Arc::ptr_eq(&self.details.borrow(), details) {
            return;
        }
        *self.details.borrow_mut() = Arc::clone(details);
        self.refresh();
    }

    fn refresh(&self) {
        let offered = offered_macros(&self.all.borrow(), &self.details.borrow(), &self.hidden.borrow());
        let visible: Vec<SharedString> =
            offered.iter().filter(|offer| offer.visible).map(|offer| SharedString::from(offer.name.as_str())).collect();
        replace_rows(&self.visible, visible);
        let items: Vec<MacroItem> = offered
            .iter()
            .map(|offer| MacroItem { name: SharedString::from(offer.name.as_str()), visible: offer.visible })
            .collect();
        replace_rows(&self.items, items);
    }
}

fn run_item(run: &MacroRun) -> MacroRunItem {
    let (state, error) = match &run.outcome {
        RunOutcome::Running => (MacroRunState::Running, ""),
        RunOutcome::Done => (MacroRunState::Done, ""),
        RunOutcome::Failed(reason) => (MacroRunState::Failed, reason.as_str()),
    };
    MacroRunItem {
        state,
        script: SharedString::from(run.script.as_str()),
        started: format::unix_clock(run.started).into(),
        duration: run.duration().map(format::duration).unwrap_or_default().into(),
        error: SharedString::from(error),
    }
}
