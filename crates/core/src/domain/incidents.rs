//! Incidents: everything that happened to the printer and is worth telling a person, in one log with one life cycle.
//!
//! - A **fault** is a state with an end the printer reports (see [`crate::domain::faults`]): it stands as long as the
//!   state lasts and stays in the log afterwards, so that whoever comes back to the printer sees what happened.
//! - An **error** is an event, such as a `!!` response or a rejected command; it is unread until the list is opened.
//! - An **information** is an event as well (`//` responses, notes of the app); it is unread until the list is opened
//!   or the state it belongs to ends. Feedback on what a person just did is never unread.
//!
//! Only the newest incident is "the event" the status bar shows for a while; the app runs the timer.

use std::collections::VecDeque;
use std::time::Duration;

use crate::domain::faults::{Fault, FaultKind};

const LIMIT: usize = 50;
/// Klipper reports a rejected command twice: as RPC error and as `!!` response. A response this close to the
/// failure and repeating its reason is the same event.
const DUPLICATE_WINDOW_SECONDS: f64 = 2.0;
/// An error, and a fault when it begins, stay in the status bar for this long.
const ERROR_DISPLAY_TIME: Duration = Duration::from_secs(15);
/// An information stays in the status bar for a base time plus the time to read it.
const INFO_BASE: Duration = Duration::from_secs(5);
/// Relaxed reading is about 13 to 15 characters per second.
const INFO_PER_CHARACTER: Duration = Duration::from_millis(70);
const MAX_INFO_DISPLAY: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Fault,
    Error,
    Info,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Subject {
    Fault(Fault),
    /// The text of an error or an information; a multi-line message stays one incident.
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Incident {
    pub id: u64,
    pub category: Category,
    pub subject: Subject,
    /// Unix time in seconds when it began.
    pub time: f64,
    /// When a fault's state ended; `None` while it stands, and for errors and informations.
    pub ended: Option<f64>,
    /// Never counts as unread: feedback on an action of this display, or an information whose state is over.
    settled: bool,
}

impl Incident {
    pub fn fault(&self) -> Option<&Fault> {
        match &self.subject {
            Subject::Fault(fault) => Some(fault),
            Subject::Text(_) => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match &self.subject {
            Subject::Text(text) => Some(text),
            Subject::Fault(_) => None,
        }
    }

    /// A fault whose state has not ended yet.
    pub fn is_standing(&self) -> bool {
        self.category == Category::Fault && self.ended.is_none()
    }

    /// How long the status bar shows it as the event.
    pub fn display_time(&self) -> Duration {
        match &self.subject {
            Subject::Text(text) if self.category == Category::Info => info_display_time(text),
            _ => ERROR_DISPLAY_TIME,
        }
    }
}

/// Unread errors and informations, counted apart: the badge shows the more serious kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Unread {
    pub errors: usize,
    pub infos: usize,
}

impl Unread {
    pub fn total(self) -> usize {
        self.errors + self.infos
    }
}

#[derive(Debug, Default)]
pub struct IncidentLog {
    incidents: VecDeque<Incident>,
    next_id: u64,
    /// The incident the status bar shows; it runs out by itself.
    event: Option<u64>,
    /// Incidents with a smaller id were there when the list was closed last: they are "Earlier", the rest is "New".
    boundary: u64,
    list_open: bool,
    /// The standing faults in the order of importance of the last [`IncidentLog::sync_faults`].
    standing: Vec<u64>,
    /// Reason of the last rejected command, to recognise its duplicate response.
    last_failure: Option<(String, f64)>,
    /// The last `!!` response (id, text, time): it may arrive before the rejection it repeats.
    last_error_response: Option<(u64, String, f64)>,
    /// The state the status bar names (connection, Klipper and print), to notice a change.
    state: Option<String>,
}

impl IncidentLog {
    fn push(&mut self, category: Category, subject: Subject, now: f64, settled: bool) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.incidents.push_back(Incident { id, category, subject, time: now, ended: None, settled });
        if self.incidents.len() > LIMIT
            && let Some(oldest) = self.incidents.iter().position(|incident| !incident.is_standing())
        {
            self.incidents.remove(oldest);
        }
        id
    }

    fn push_event(&mut self, category: Category, text: &str, now: f64, settled: bool) -> u64 {
        let id = self.push(category, Subject::Text(text.to_owned()), now, settled);
        self.event = Some(id);
        id
    }

    /// An error the app reports, such as a command it could not send. Returns its id; it is the event now.
    pub fn add_error(&mut self, text: &str, now: f64) -> u64 {
        self.push_event(Category::Error, text, now, false)
    }

    /// An information worth reading later. Returns its id; it is the event now.
    pub fn add_info(&mut self, text: &str, now: f64) -> u64 {
        self.push_event(Category::Info, text, now, false)
    }

    /// Feedback on what a person just did here ("Planned pauses cleared"): shown, listed, never unread.
    pub fn add_feedback(&mut self, text: &str, now: f64) -> u64 {
        self.push_event(Category::Info, text, now, true)
    }

    /// An information for the list only: the status bar keeps showing what it shows. Returns its id.
    pub fn add_info_quietly(&mut self, text: &str, now: f64) -> u64 {
        self.push(Category::Info, Subject::Text(text.to_owned()), now, false)
    }

    /// A command of the app was rejected. `shown` goes to the list, `reason` is what Klipper repeats in its response.
    /// Klipper's response may come first: then the rejection, which also names the command, replaces it.
    pub fn add_command_failure(&mut self, shown: &str, reason: &str, now: f64) -> u64 {
        if let Some((id, _, _)) = self
            .last_error_response
            .take()
            .filter(|(_, text, time)| now - time <= DUPLICATE_WINDOW_SECONDS && text.contains(reason))
        {
            self.incidents.retain(|incident| incident.id != id);
        }
        self.last_failure = Some((reason.to_owned(), now));
        self.add_error(shown, now)
    }

    /// A `!!` or `//` response of Klipper. Returns `None` for an error that repeats the failure just reported.
    pub fn add_response(&mut self, text: &str, is_error: bool, now: f64) -> Option<u64> {
        if !is_error {
            return Some(self.add_info(text, now));
        }
        if self.repeats_last_failure(text, now) {
            return None;
        }
        let id = self.add_error(text, now);
        self.last_error_response = Some((id, text.to_owned(), now));
        Some(id)
    }

    fn repeats_last_failure(&self, text: &str, now: f64) -> bool {
        self.last_failure
            .as_ref()
            .is_some_and(|(reason, time)| now - time <= DUPLICATE_WINDOW_SECONDS && text.contains(reason.as_str()))
    }

    /// Brings the faults up to date with what stands now, most important first. A fault that begins becomes the
    /// event; one whose state ended stays in the list with its end. Returns the id of a fault that began, if any.
    pub fn sync_faults(&mut self, faults: &[Fault], now: f64) -> Option<u64> {
        let mut began = None;
        let mut standing = Vec::with_capacity(faults.len());
        for fault in faults {
            let kind = fault.kind();
            match self.standing_of_kind(kind) {
                Some(incident) => {
                    incident.subject = Subject::Fault(fault.clone());
                    standing.push(incident.id);
                }
                None => {
                    let id = self.push(Category::Fault, Subject::Fault(fault.clone()), now, false);
                    self.event = Some(id);
                    began = Some(id);
                    standing.push(id);
                }
            }
        }
        for incident in self.incidents.iter_mut().filter(|incident| incident.is_standing()) {
            if !standing.contains(&incident.id) {
                incident.ended = Some(now);
            }
        }
        if self.event().is_some_and(|event| event.category == Category::Fault && !event.is_standing()) {
            self.event = None;
        }
        self.standing = standing;
        began
    }

    fn standing_of_kind(&mut self, kind: FaultKind) -> Option<&mut Incident> {
        self.incidents
            .iter_mut()
            .find(|incident| incident.is_standing() && incident.fault().is_some_and(|fault| fault.kind() == kind))
    }

    /// The faults that stand now, most important first.
    pub fn standing(&self) -> impl Iterator<Item = &Incident> {
        self.standing.iter().filter_map(|id| self.find(*id))
    }

    fn find(&self, id: u64) -> Option<&Incident> {
        self.incidents.iter().find(|incident| incident.id == id)
    }

    /// The incident the status bar shows, if any.
    pub fn event(&self) -> Option<&Incident> {
        self.find(self.event?)
    }

    pub fn get(&self, id: u64) -> Option<&Incident> {
        self.find(id)
    }

    /// All incidents, newest first.
    pub fn incidents(&self) -> impl Iterator<Item = &Incident> {
        self.incidents.iter().rev()
    }

    /// Whether the list shows the incident under "New": it came after the list was closed last.
    pub fn is_new(&self, incident: &Incident) -> bool {
        incident.id >= self.boundary
    }

    /// Whether it still asks to be looked at: a fault that stands, or an error or information nobody has seen yet.
    /// Seen and past ones are plain history.
    pub fn needs_attention(&self, incident: &Incident) -> bool {
        match incident.category {
            Category::Fault => incident.is_standing(),
            Category::Error | Category::Info => !incident.settled && self.is_new(incident),
        }
    }

    /// Errors and informations that came since the list was closed last. None while it is open, as everything is
    /// seen then. Faults are not counted: they stand in the status bar themselves.
    pub fn unread(&self) -> Unread {
        if self.list_open {
            return Unread::default();
        }
        let mut unread = Unread::default();
        for incident in self.incidents.iter().filter(|incident| self.needs_attention(incident)) {
            match incident.category {
                Category::Error => unread.errors += 1,
                Category::Info => unread.infos += 1,
                Category::Fault => {}
            }
        }
        unread
    }

    /// The list was opened: what came since it was closed last is "New" until it closes again.
    pub fn open_list(&mut self) {
        self.list_open = true;
    }

    /// The list was closed: everything in it is "Earlier" from now on.
    pub fn close_list(&mut self) {
        self.list_open = false;
        self.boundary = self.next_id;
    }

    /// The incident stood on the screen somewhere else as well (the run card of a macro): it stays in the list but no
    /// longer counts as unread.
    pub fn settle(&mut self, id: u64) {
        if let Some(incident) = self.incidents.iter_mut().find(|incident| incident.id == id) {
            incident.settled = true;
        }
    }

    /// The event ran out of display time. An older one does not take its place: the counter keeps it.
    pub fn hide_event(&mut self, id: u64) {
        if self.event == Some(id) {
            self.event = None;
        }
    }

    /// Tells the log the state the status bar names. When it changes, the informations belonged to the state before:
    /// one in the status bar ends at once and none counts as unread any more; they stay in the list. Errors keep their
    /// display time and stay unread. Returns whether anything changed.
    pub fn note_state(&mut self, state: &str) -> bool {
        let changed = self.state.as_deref().is_some_and(|last| last != state);
        if self.state.as_deref() != Some(state) {
            self.state = Some(state.to_owned());
        }
        if !changed {
            return false;
        }
        if self.event().is_some_and(|event| event.category == Category::Info) {
            self.event = None;
        }
        for incident in self.incidents.iter_mut().filter(|incident| incident.category == Category::Info) {
            incident.settled = true;
        }
        true
    }

    /// Empties the list but keeps the faults that stand: their state has not ended.
    pub fn clear(&mut self) {
        self.incidents.retain(Incident::is_standing);
        if self.event().is_none() {
            self.event = None;
        }
        self.boundary = self.next_id;
    }
}

/// How long an information stays in the status bar.
fn info_display_time(text: &str) -> Duration {
    let characters = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
    (INFO_BASE + INFO_PER_CHARACTER * characters).min(MAX_INFO_DISPLAY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::pause::Interruption;

    fn texts(log: &IncidentLog) -> Vec<&str> {
        log.incidents().filter_map(Incident::text).collect()
    }

    fn event_text(log: &IncidentLog) -> Option<&str> {
        log.event().and_then(Incident::text)
    }

    fn paused() -> Fault {
        Fault::Print(Interruption { failed: false, facts: Vec::new() })
    }

    #[test]
    fn newest_message_is_the_event_and_listed_first() {
        let mut log = IncidentLog::default();
        log.add_error("first", 1.0);
        log.add_info("second", 2.0);

        assert_eq!(event_text(&log), Some("second"));
        assert_eq!(texts(&log), ["second", "first"]);
    }

    #[test]
    fn hiding_an_old_event_keeps_a_newer_one() {
        let mut log = IncidentLog::default();
        let old = log.add_info("old info", 1.0);
        log.add_error("error", 2.0);

        log.hide_event(old);

        assert_eq!(event_text(&log), Some("error"));
    }

    #[test]
    fn response_repeating_a_failure_is_skipped() {
        let mut log = IncidentLog::default();
        log.add_command_failure("G1 X10: Must home axis first", "Must home axis first", 10.0);

        assert!(log.add_response("Must home axis first: 0 0", true, 10.5).is_none());
        assert_eq!(log.incidents().count(), 1);
    }

    #[test]
    fn a_rejection_after_its_response_replaces_it() {
        let mut log = IncidentLog::default();
        log.add_response("Unknown fan 'missing'", true, 10.0);

        log.add_command_failure("SET_FAN_SPEED FAN=missing: Unknown fan 'missing'", "Unknown fan 'missing'", 10.2);

        assert_eq!(texts(&log), ["SET_FAN_SPEED FAN=missing: Unknown fan 'missing'"]);
    }

    #[test]
    fn repeat_after_the_window_is_a_new_message() {
        let mut log = IncidentLog::default();
        log.add_command_failure("G1 X10: Must home axis first", "Must home axis first", 10.0);

        assert!(log.add_response("Must home axis first", true, 20.0).is_some());
    }

    #[test]
    fn informations_are_never_skipped_as_duplicates() {
        let mut log = IncidentLog::default();
        log.add_command_failure("x", "Must home", 10.0);

        assert!(log.add_response("Must home", false, 10.1).is_some());
    }

    #[test]
    fn list_is_limited_to_the_newest_incidents_but_keeps_a_standing_fault() {
        let mut log = IncidentLog::default();
        log.sync_faults(&[Fault::AfcError], 0.0);
        for index in 0..60 {
            log.add_error(&format!("message {index}"), f64::from(index));
        }

        assert_eq!(log.incidents().count(), LIMIT);
        assert_eq!(log.standing().count(), 1);
        assert_eq!(texts(&log).last(), Some(&"message 11"));
    }

    #[test]
    fn information_time_grows_with_length_up_to_a_limit() {
        assert_eq!(info_display_time(""), Duration::from_secs(5));
        assert_eq!(info_display_time(&"x".repeat(25)), Duration::from_millis(6750));
        assert_eq!(info_display_time(&"x".repeat(1000)), Duration::from_secs(15));
    }

    #[test]
    fn errors_and_faults_display_for_fifteen_seconds() {
        let mut log = IncidentLog::default();
        let error = log.add_error("x", 1.0);
        let fault = log.sync_faults(&[Fault::AfcError], 2.0).unwrap();

        assert_eq!(log.get(error).unwrap().display_time(), Duration::from_secs(15));
        assert_eq!(log.get(fault).unwrap().display_time(), Duration::from_secs(15));
    }

    #[test]
    fn errors_and_informations_count_as_unread_apart() {
        let mut log = IncidentLog::default();
        log.add_error("fault", 1.0);
        log.add_info("note", 2.0);
        log.add_error("another fault", 3.0);

        assert_eq!(log.unread(), Unread { errors: 2, infos: 1 });
    }

    #[test]
    fn feedback_on_an_own_action_is_shown_but_never_unread() {
        let mut log = IncidentLog::default();
        log.add_feedback("Planned pauses cleared", 1.0);

        assert_eq!(event_text(&log), Some("Planned pauses cleared"));
        assert_eq!(log.unread().total(), 0);
    }

    #[test]
    fn what_stood_on_the_screen_elsewhere_stays_listed_but_is_not_unread() {
        let mut log = IncidentLog::default();
        let error = log.add_response("Must home axis first", true, 1.0).unwrap();
        let info = log.add_response("Heat soak started", false, 2.0).unwrap();
        log.add_info("other", 3.0);
        log.settle(error);
        log.settle(info);

        assert_eq!(log.unread(), Unread { errors: 0, infos: 1 });
        assert_eq!(log.incidents().count(), 3);
    }

    #[test]
    fn a_state_change_ends_and_settles_informations_but_not_errors() {
        let mut log = IncidentLog::default();
        assert!(!log.note_state("ready printing"), "the first state is only remembered");
        log.add_info("Pause at the next layer sent", 1.0);
        assert!(!log.note_state("ready printing"));
        assert!(log.event().is_some());

        assert!(log.note_state("ready cancelled"));

        assert!(log.event().is_none());
        assert_eq!(log.incidents().count(), 1, "the information stays in the list");
        assert_eq!(log.unread().infos, 0);
        log.add_error("Move out of range", 2.0);
        assert!(log.note_state("ready standby"));
        assert_eq!(log.event().map(|event| event.category), Some(Category::Error));
        assert_eq!(log.unread().errors, 1);
    }

    #[test]
    fn opening_the_list_resets_the_counter_and_closing_makes_everything_earlier() {
        let mut log = IncidentLog::default();
        log.add_error("fault", 1.0);

        log.open_list();
        assert_eq!(log.unread().total(), 0);
        assert!(log.incidents().all(|incident| log.is_new(incident)));
        log.close_list();

        assert!(log.incidents().all(|incident| !log.is_new(incident) && !log.needs_attention(incident)));
        log.add_error("after", 2.0);
        assert_eq!(log.unread().errors, 1);
    }

    #[test]
    fn a_fault_that_begins_is_the_event_and_stands_until_its_state_ends() {
        let mut log = IncidentLog::default();

        let id = log.sync_faults(&[paused()], 1.0);

        assert_eq!(log.event().map(|event| event.id), id);
        assert_eq!(log.standing().count(), 1);
        assert_eq!(log.unread().total(), 0, "a fault stands in the status bar, it is not counted");

        assert_eq!(log.sync_faults(&[paused()], 2.0), None, "the same fault does not begin again");
        log.sync_faults(&[], 3.0);

        assert_eq!(log.standing().count(), 0);
        assert!(log.event().is_none(), "the event of an ended fault leaves the status bar");
        let ended = log.incidents().next().unwrap();
        assert_eq!(ended.ended, Some(3.0));
        assert!(!log.needs_attention(ended), "a past fault is history");
    }

    #[test]
    fn a_changing_fault_keeps_its_incident_and_takes_the_new_facts() {
        let mut log = IncidentLog::default();
        log.sync_faults(&[Fault::Klipper { shutdown: true, message: "first".into() }], 1.0);
        log.sync_faults(&[Fault::Klipper { shutdown: true, message: "second".into() }], 2.0);

        assert_eq!(log.incidents().count(), 1);
        assert_eq!(
            log.standing().next().and_then(Incident::fault),
            Some(&Fault::Klipper { shutdown: true, message: "second".into() })
        );
    }

    #[test]
    fn standing_faults_follow_the_order_of_importance() {
        let mut log = IncidentLog::default();
        log.sync_faults(&[Fault::SpoolmanOffline], 1.0);
        log.sync_faults(&[paused(), Fault::SpoolmanOffline], 2.0);

        let kinds: Vec<FaultKind> = log.standing().filter_map(Incident::fault).map(Fault::kind).collect();

        assert_eq!(kinds, [FaultKind::PrintPaused, FaultKind::SpoolmanOffline]);
    }

    #[test]
    fn the_same_fault_after_its_end_is_a_new_incident() {
        let mut log = IncidentLog::default();
        log.sync_faults(&[Fault::AfcError], 1.0);
        log.sync_faults(&[], 2.0);

        assert!(log.sync_faults(&[Fault::AfcError], 3.0).is_some());
        assert_eq!(log.incidents().count(), 2);
    }

    #[test]
    fn clear_keeps_the_standing_faults_only() {
        let mut log = IncidentLog::default();
        log.sync_faults(&[Fault::AfcError], 1.0);
        log.add_error("error", 2.0);

        log.clear();

        assert_eq!(log.incidents().count(), 1);
        assert_eq!(log.standing().count(), 1);
        assert_eq!(log.unread().total(), 0);
    }

    #[test]
    fn a_quiet_information_is_listed_without_replacing_the_event() {
        let mut log = IncidentLog::default();
        log.add_error("fault", 1.0);

        log.add_info_quietly("note", 2.0);

        assert_eq!(event_text(&log), Some("fault"));
        assert_eq!(log.unread(), Unread { errors: 1, infos: 1 });
    }
}
