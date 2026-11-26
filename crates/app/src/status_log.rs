//! Layout of the message list in the flyout: the faults that stand first, then what is new, then the rest.
//!
//! Rows are laid out by hand, as the list scrolls and its sections vary; the log itself lives in the core
//! ([`IncidentLog`]).

use sailhand_core::domain::incidents::{Incident, IncidentLog};

/// Layout of the flyout list, in logical pixels: a message row has the symbol before and the time after its text.
const ROW_GAP: f32 = 6.0;
/// Height of a section heading ("Now", "New", "Earlier") including the gap below it.
const SECTION_HEIGHT: f32 = 24.0;
const LINE_HEIGHT: f32 = 22.0;
const ROW_PADDING: f32 = 20.0;
/// Roughly what fits into the width of the flyout at the normal font size, beside the symbol and the time.
const CHARACTERS_PER_LINE: usize = 40;
/// A standing fault has "since" before its time and the arrow of its jump after it.
const CHARACTERS_PER_LINE_STANDING: usize = 30;
const MAX_LINES: usize = 10;

/// Height of a message row: one line per wrapped line of the text, but never below the minimum.
pub fn row_height(text: &str, standing: bool) -> f32 {
    let per_line = if standing { CHARACTERS_PER_LINE_STANDING } else { CHARACTERS_PER_LINE };
    let lines: usize = text.lines().map(|line| line.chars().count().div_ceil(per_line).max(1)).sum();
    lines.clamp(1, MAX_LINES) as f32 * LINE_HEIGHT + ROW_PADDING
}

/// An incident of the flyout list with its place.
#[derive(Debug, PartialEq)]
pub struct Placed<'a> {
    pub incident: &'a Incident,
    /// Distance from the top of the list, in logical pixels.
    pub top: f32,
    pub height: f32,
}

/// Where each section heading sits; `None` for a section without rows.
#[derive(Debug, Default, PartialEq)]
pub struct Headings {
    pub now: Option<f32>,
    pub new: Option<f32>,
    pub earlier: Option<f32>,
}

#[derive(Debug, PartialEq)]
pub struct Layout<'a> {
    pub rows: Vec<Placed<'a>>,
    /// Height of the whole list.
    pub height: f32,
    pub headings: Headings,
}

/// Lays out the list: the standing faults under "Now", then the rest newest first under "New" and "Earlier".
/// `text` gives the shown text of an incident, which decides its height.
pub fn layout<'a>(log: &'a IncidentLog, text: impl Fn(&Incident) -> String) -> Layout<'a> {
    let standing = log.standing();
    let new = log.incidents().filter(|incident| !incident.is_standing() && log.is_new(incident));
    let earlier = log.incidents().filter(|incident| !incident.is_standing() && !log.is_new(incident));
    let mut rows = Vec::new();
    let mut top = 0.0;
    let mut place_section = |incidents: &mut dyn Iterator<Item = &'a Incident>| -> Option<f32> {
        let mut heading = None;
        for incident in incidents {
            if heading.is_none() {
                heading = Some(top);
                top += SECTION_HEIGHT;
            }
            let height = row_height(&text(incident), incident.is_standing());
            rows.push(Placed { incident, top, height });
            top += height + ROW_GAP;
        }
        heading
    };
    let headings = Headings {
        now: place_section(&mut standing.into_iter()),
        new: place_section(&mut new.into_iter()),
        earlier: place_section(&mut earlier.into_iter()),
    };
    Layout { rows, height: top, headings }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sailhand_core::domain::faults::Fault;

    fn shown(incident: &Incident) -> String {
        incident.text().unwrap_or("fault").to_owned()
    }

    #[test]
    fn standing_faults_come_first_then_new_then_earlier() {
        let mut log = IncidentLog::default();
        log.add_info("old", 1.0);
        log.open_list();
        log.close_list();
        log.add_info("fresh", 2.0);
        log.sync_faults(&[Fault::AfcError], 3.0);

        let layout = layout(&log, shown);

        let row = row_height("fresh", false);
        let order: Vec<String> = layout.rows.iter().map(|placed| shown(placed.incident)).collect();
        assert_eq!(order, ["fault", "fresh", "old"]);
        let tops: Vec<f32> = layout.rows.iter().map(|placed| placed.top).collect();
        let step = row + ROW_GAP + SECTION_HEIGHT;
        assert_eq!(tops, [SECTION_HEIGHT, SECTION_HEIGHT + step, SECTION_HEIGHT + 2.0 * step]);
        assert_eq!(layout.headings, Headings { now: Some(0.0), new: Some(step), earlier: Some(2.0 * step) });
        assert_eq!(layout.height, 3.0 * (SECTION_HEIGHT + row + ROW_GAP));
    }

    #[test]
    fn an_ended_fault_moves_to_the_history() {
        let mut log = IncidentLog::default();
        log.sync_faults(&[Fault::AfcError], 1.0);
        log.sync_faults(&[], 2.0);

        let layout = layout(&log, shown);

        assert_eq!(layout.headings, Headings { now: None, new: Some(0.0), earlier: None });
    }

    #[test]
    fn the_layout_of_an_empty_log_is_empty() {
        let log = IncidentLog::default();

        let layout = layout(&log, shown);

        assert!(layout.rows.is_empty());
        assert_eq!((layout.height, layout.headings), (0.0, Headings::default()));
    }

    #[test]
    fn row_height_grows_with_wrapped_lines_up_to_a_limit() {
        let frame = ROW_PADDING;
        assert_eq!(row_height("short", false), 22.0 + frame);
        assert_eq!(row_height("line one\nline two\nline three", false), 3.0 * 22.0 + frame);
        assert_eq!(row_height(&"x".repeat(100), false), 3.0 * 22.0 + frame);
        assert_eq!(row_height(&"x".repeat(100), true), 4.0 * 22.0 + frame);
        assert_eq!(row_height(&"x\n".repeat(30), false), 10.0 * 22.0 + frame);
    }
}
