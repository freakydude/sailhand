//! The status bar and its flyout: the event the bar shows, the fault that stands, the counter of unread messages and
//! the list of all incidents. The log and its rules are in the core ([`IncidentLog`]); this puts them on screen.

use crate::items::replace_rows;
use crate::status_log::layout;
use crate::{AppWindow, MessageItem, Place, Printer, Severity, format, view};
use sailhand_core::domain::faults::{self, Fault, FaultLevel};
use sailhand_core::domain::incidents::{Category, Incident, IncidentLog, Target};
use sailhand_core::domain::routines::Routine;
use sailhand_core::sources::moonraker_server::gcode_store::ConsoleLine;
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

/// Shared with the timer that hides the event again, hence the `Rc`.
pub struct StatusView {
    window: slint::Weak<AppWindow>,
    log: RefCell<IncidentLog>,
    messages: Rc<VecModel<MessageItem>>,
    /// Only the newest event is shown, hence one timer.
    timer: Timer,
}

impl StatusView {
    pub fn new(window: &AppWindow) -> Rc<Self> {
        let view = Rc::new(Self {
            window: window.as_weak(),
            log: RefCell::default(),
            messages: Rc::default(),
            timer: Timer::default(),
        });
        window.global::<Printer>().set_messages(ModelRc::from(view.messages.clone()));
        view
    }

    /// Puts the log on screen: the flyout list with its sections, the event, the standing fault and the counter.
    pub fn refresh(&self) {
        let log = self.log.borrow();
        let layout = layout(&log, incident_text);
        let items: Vec<MessageItem> = layout
            .rows
            .iter()
            .map(|placed| MessageItem {
                id: i32::try_from(placed.incident.id).unwrap_or(i32::MAX),
                text: incident_text(placed.incident).into(),
                time: format::unix_clock(placed.incident.time).into(),
                severity: severity(placed.incident),
                tinted: log.needs_attention(placed.incident),
                standing: placed.incident.is_standing(),
                place: place(placed.incident),
                place_name: place_name(placed.incident).into(),
                top: placed.top,
                height: placed.height,
            })
            .collect();
        replace_rows(&self.messages, items);
        let Some(window) = self.window.upgrade() else { return };
        let printer = window.global::<Printer>();
        printer.set_messages_height(layout.height);
        printer.set_messages_has_now(layout.headings.now.is_some());
        printer.set_messages_has_new(layout.headings.new.is_some());
        printer.set_messages_new_top(layout.headings.new.unwrap_or_default());
        printer.set_messages_has_earlier(layout.headings.earlier.is_some());
        printer.set_messages_earlier_top(layout.headings.earlier.unwrap_or_default());
        let unread = log.unread();
        printer.set_unread_count(i32::try_from(unread.total()).unwrap_or(i32::MAX));
        printer.set_unread_severity(if unread.errors > 0 { Severity::Error } else { Severity::Info });
        let event = log.event();
        printer.set_status_message(event.map(headline).unwrap_or_default());
        printer.set_status_message_severity(event.map_or(Severity::Info, severity));
        printer.set_status_message_visible(event.is_some());
        show_fault(&printer, &log);
    }

    /// Notes the summary of connection, Klipper and print state; a change of it settles the informations.
    pub fn note_state(&self, state: &str) {
        if self.log.borrow_mut().note_state(state) {
            self.refresh();
        }
    }

    /// Brings the standing faults up to date; one that begins is shown over the whole status bar for a while.
    pub fn sync_faults(self: &Rc<Self>, standing: &[Fault]) {
        let began = self.log.borrow_mut().sync_faults(standing, now_seconds());
        match began {
            Some(id) => self.show_event(id),
            None => self.refresh(),
        }
    }

    /// Feedback on an action of this display (not unread), or an error the app reports.
    pub fn notice(self: &Rc<Self>, text: &str, is_error: bool) {
        let mut log = self.log.borrow_mut();
        let id = if is_error { log.add_error(text, now_seconds()) } else { log.add_feedback(text, now_seconds()) };
        drop(log);
        self.show_event(id);
    }

    /// A command Klipper or Moonraker rejected; its duplicate response is recognised by the reason. `seen`: it stood
    /// on the screen elsewhere (the run card of a macro), so it is not unread. `routine`: the command belonged to one.
    pub fn command_failed(self: &Rc<Self>, shown: &str, reason: &str, seen: bool, routine: Option<Routine>) {
        let mut log = self.log.borrow_mut();
        let id = log.add_command_failure(shown, reason, now_seconds());
        if seen {
            log.settle(id);
        }
        if let Some(routine) = routine {
            log.set_routine(id, routine);
        }
        drop(log);
        self.show_event(id);
    }

    /// A `!!` (error) or `//` (information) response of Klipper. Other responses stay in the console.
    /// While a print runs, informations only go to the list: Klipper sends many of them and the status bar would flicker.
    /// `seen`: the line stood on the screen elsewhere (the run card of a macro), so it is not unread. `routine`: a
    /// routine of this display was running when it came.
    pub fn console_response(self: &Rc<Self>, line: &ConsoleLine, printing: bool, seen: bool, routine: Option<Routine>) {
        let is_error = line.is_error();
        if !is_error && !line.is_information() {
            return;
        }
        let text = line.text();
        let mut log = self.log.borrow_mut();
        let shown = is_error || !printing;
        let added = if shown {
            log.add_response(&text, is_error, now_seconds())
        } else {
            Some(log.add_response_quietly(&text, now_seconds()))
        };
        if let Some(id) = added {
            if seen {
                log.settle(id);
            }
            if let Some(routine) = routine {
                log.set_routine(id, routine);
            }
        }
        drop(log);
        match added {
            Some(id) if shown => self.show_event(id),
            _ => self.refresh(),
        }
    }

    /// Where in the console the incident with this id happened: its time and the text to find there.
    pub fn console_anchor(&self, id: u64) -> Option<(f64, Option<String>)> {
        match self.log.borrow().get(id)?.target() {
            Target::Console { time, text } => Some((time, text.map(str::to_owned))),
            Target::Place(_) | Target::Routine(_) => None,
        }
    }

    pub fn open_list(&self) {
        self.log.borrow_mut().open_list();
        self.refresh();
    }

    pub fn close_list(&self) {
        self.log.borrow_mut().close_list();
        self.refresh();
    }

    pub fn clear(&self) {
        self.log.borrow_mut().clear();
        self.refresh();
    }

    /// Shows an incident in the status bar as the event; it leaves again after its display time.
    fn show_event(self: &Rc<Self>, id: u64) {
        let Some(duration) = self.log.borrow().get(id).map(Incident::display_time) else { return };
        let view = Rc::downgrade(self);
        self.timer.start(TimerMode::SingleShot, duration, move || {
            let Some(view) = view.upgrade() else { return };
            view.log.borrow_mut().hide_event(id);
            view.refresh();
        });
        self.refresh();
    }
}

/// The chip of the most important standing fault and how many more there are.
fn show_fault(printer: &Printer, log: &IncidentLog) {
    let mut standing = log.standing().filter_map(Incident::fault);
    let first = standing.next();
    printer.set_fault_visible(first.is_some());
    printer.set_fault_label(first.map(view::fault_label).unwrap_or_default().into());
    printer.set_fault_severity(first.map_or(Severity::Error, |fault| fault_severity(fault.level())));
    printer.set_fault_place(first.map_or(Place::None, |fault| place_of(&fault.place())));
    printer.set_fault_is_state(first.is_some_and(Fault::is_machine_state));
    printer.set_fault_more(i32::try_from(standing.count()).unwrap_or(i32::MAX));
}

/// The whole text of an incident as the list shows it.
fn incident_text(incident: &Incident) -> String {
    match (incident.fault(), incident.text()) {
        (Some(fault), _) => view::fault_text(fault),
        (None, Some(text)) => text.to_owned(),
        (None, None) => String::new(),
    }
}

/// The first line, which the status bar shows.
fn headline(incident: &Incident) -> SharedString {
    incident_text(incident).lines().next().unwrap_or_default().into()
}

fn severity(incident: &Incident) -> Severity {
    match incident.category {
        Category::Fault => incident.fault().map_or(Severity::Error, |fault| fault_severity(fault.level())),
        Category::Error => Severity::Error,
        Category::Info => Severity::Info,
    }
}

fn fault_severity(level: FaultLevel) -> Severity {
    match level {
        FaultLevel::Attention => Severity::Attention,
        FaultLevel::Error => Severity::Error,
    }
}

/// Where a tap on the incident leads ([`Incident::target`]).
fn place(incident: &Incident) -> Place {
    match incident.target() {
        Target::Place(place) => place_of(&place),
        Target::Routine(Routine::Macro(_)) => Place::RoutineMacro,
        Target::Routine(Routine::Bed) => Place::RoutineBed,
        Target::Routine(Routine::Pid(_)) => Place::RoutinePid,
        Target::Console { .. } => Place::Console,
    }
}

/// Which one at that place: the macro, the heater, or for the console the incident itself (its id).
fn place_name(incident: &Incident) -> String {
    match incident.target() {
        Target::Routine(Routine::Macro(name) | Routine::Pid(name)) => name.clone(),
        Target::Console { .. } => incident.id.to_string(),
        Target::Place(_) | Target::Routine(Routine::Bed) => String::new(),
    }
}

fn place_of(place: &faults::Place) -> Place {
    match place {
        faults::Place::Connection => Place::Connection,
        faults::Place::Print => Place::Print,
        faults::Place::AfcService => Place::AfcService,
        faults::Place::ChangerService => Place::ChangerService,
        faults::Place::Spools => Place::Spools,
    }
}

fn now_seconds() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |elapsed| elapsed.as_secs_f64())
}
