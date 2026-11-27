//! The console list: the lines of Klipper's console as rows of fixed height, and the filter for temperature reports.

use crate::{ConsoleItem, Printer, format};
use sailhand_core::sources::moonraker_server::gcode_store::{ConsoleKind, ConsoleLine};
use slint::{Model, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The oldest rows are dropped beyond this many.
const CONSOLE_LIMIT: usize = 300;

pub struct ConsoleView {
    rows: Rc<VecModel<ConsoleItem>>,
    clock: RefCell<ClockRows>,
    /// Leave Klipper's temperature reports out of the console (setting).
    hide_temperature_reports: Cell<bool>,
}

impl Default for ConsoleView {
    fn default() -> Self {
        Self { rows: Rc::default(), clock: RefCell::default(), hide_temperature_reports: Cell::new(true) }
    }
}

impl ConsoleView {
    pub fn bind(&self, printer: &Printer) {
        printer.set_console(ModelRc::from(self.rows.clone()));
    }

    /// Shows the history instead of the rows so far.
    pub fn show_history(&self, lines: &[ConsoleLine]) {
        let mut clock = ClockRows::default();
        let items: Vec<ConsoleItem> =
            lines.iter().filter(|line| self.shows(line)).flat_map(|line| clock.rows(line)).collect();
        let skip = items.len().saturating_sub(CONSOLE_LIMIT);
        let mut items: Vec<ConsoleItem> = items.into_iter().skip(skip).collect();
        if let Some(first) = items.first_mut() {
            first.show_time = true;
        }
        self.rows.set_vec(items);
        *self.clock.borrow_mut() = clock;
    }

    /// Appends the rows of a line that arrived live.
    pub fn push(&self, line: &ConsoleLine) {
        let shown = if self.shows(line) { self.clock.borrow_mut().rows(line) } else { Vec::new() };
        for item in shown {
            if self.rows.row_count() >= CONSOLE_LIMIT {
                self.rows.remove(0);
                show_time_on_top(&self.rows);
            }
            self.rows.push(item);
        }
    }

    /// Empties the shown log only.
    pub fn clear(&self) {
        self.rows.set_vec(Vec::new());
        *self.clock.borrow_mut() = ClockRows::default();
    }

    /// Whether the setting changed.
    pub fn set_hide_temperature_reports(&self, hide: bool) -> bool {
        self.hide_temperature_reports.replace(hide) != hide
    }

    fn shows(&self, line: &ConsoleLine) -> bool {
        !(self.hide_temperature_reports.get() && line.is_temperature_report())
    }
}

/// The top row lost the row above it that showed its time.
fn show_time_on_top(rows: &VecModel<ConsoleItem>) {
    if let Some(mut top) = rows.row_data(0).filter(|top| !top.show_time) {
        top.show_time = true;
        rows.set_row_data(0, top);
    }
}

/// Turns console lines into rows of a list and shows the time only where it changes: the rows of the same second
/// below keep the full width.
#[derive(Default)]
pub(crate) struct ClockRows {
    last_shown: Option<String>,
}

impl ClockRows {
    /// One row per message line: the list has fixed row heights, so a line break inside a row would overlap its
    /// neighbours. Only the first row of a message can show the time.
    pub(crate) fn rows(&mut self, line: &ConsoleLine) -> Vec<ConsoleItem> {
        let time = format::unix_clock(line.time);
        let new_second = self.last_shown.as_deref() != Some(time.as_str());
        let is_command = line.kind == ConsoleKind::Command;
        let is_error = line.is_error();
        let text = line.text();
        let mut rows: Vec<&str> = text.lines().collect();
        if rows.is_empty() {
            rows.push("");
        }
        let items = rows
            .into_iter()
            .enumerate()
            .map(|(index, text)| ConsoleItem {
                text: text.into(),
                time: time.as_str().into(),
                show_time: index == 0 && new_second,
                is_command,
                is_error,
            })
            .collect();
        self.last_shown = Some(time);
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: ConsoleKind, message: &str) -> ConsoleLine {
        ConsoleLine { kind, message: message.to_owned(), time: 0.0 }
    }

    #[test]
    fn multi_line_message_becomes_one_row_per_line() {
        let rows = ClockRows::default().rows(&line(ConsoleKind::Response, "first\nsecond\r\nthird"));
        let texts: Vec<&str> = rows.iter().map(|row| row.text.as_str()).collect();
        assert_eq!(texts, ["first", "second", "third"]);
        assert!(rows[0].show_time && !rows[1].show_time && !rows[2].show_time);
    }

    #[test]
    fn the_time_shows_only_where_the_second_changes() {
        let at = |time: f64, message: &str| ConsoleLine { kind: ConsoleKind::Response, message: message.into(), time };
        let mut clock = ClockRows::default();
        let shown: Vec<bool> = [at(10.2, "a"), at(10.7, "b"), at(11.1, "c"), at(11.1, "d")]
            .iter()
            .flat_map(|line| clock.rows(line))
            .map(|row| row.show_time)
            .collect();
        assert_eq!(shown, [true, false, true, false]);
    }

    #[test]
    fn the_top_row_shows_its_time_when_older_rows_are_dropped() {
        let view = ConsoleView::default();
        for index in 0..=CONSOLE_LIMIT {
            view.push(&ConsoleLine { kind: ConsoleKind::Response, message: format!("{index}"), time: 0.0 });
        }
        assert!(view.rows.row_data(0).is_some_and(|top| top.text == "1" && top.show_time));
        assert!(view.rows.row_data(1).is_some_and(|row| !row.show_time));
    }

    #[test]
    fn empty_message_keeps_one_row() {
        let rows = ClockRows::default().rows(&line(ConsoleKind::Response, ""));
        assert_eq!(rows.len(), 1);
        assert!(rows[0].text.is_empty());
    }

    #[test]
    fn plugin_markup_does_not_reach_the_console() {
        let rows = ClockRows::default()
            .rows(&line(ConsoleKind::Response, r#"// <b><span style="color:#87CEEB">EMPTY</span></b>&nbsp;gate"#));
        assert_eq!(rows[0].text, "EMPTY gate");
    }

    #[test]
    fn error_flag_applies_to_every_row() {
        let rows = ClockRows::default().rows(&line(ConsoleKind::Response, "!! Lost connection\nsee log"));
        assert_eq!(rows[0].text, "Lost connection");
        assert!(rows.iter().all(|row| row.is_error));
        assert!(!rows.iter().any(|row| row.is_command));
    }
}
