//! Macro prompts: the dialogs a Klipper macro asks the client to show with `RESPOND TYPE=command
//! MSG="action:prompt_begin ..."`. Klipper itself only forwards the lines as `// action:prompt_*` responses; the
//! protocol is the one Mainsail implements (`src/components/dialogs/TheMacroPrompt.vue`, `MacroPromptButton.vue`,
//! `MacroPromptFooterButton.vue`, `src/store/server/actions.ts` in mainsail-crew/mainsail), and Fluidd and
//! KlipperScreen follow it.
//!
//! The lines of one prompt:
//!
//! ```text
//! // action:prompt_begin <title>
//! // action:prompt_text <text>
//! // action:prompt_button <label>|<gcode>|<style>
//! // action:prompt_button_group_start
//! // action:prompt_button_group_end
//! // action:prompt_footer_button <label>|<gcode>|<style>
//! // action:prompt_show
//! // action:prompt_end
//! ```
//!
//! [`PromptTracker`] reads the lines in order and keeps the prompt that is open. Like Mainsail's it shows a prompt
//! only from `prompt_show` on, with what came between its `prompt_begin` and the last `prompt_show`; a new
//! `prompt_begin` drops the one before, `prompt_end` closes it (also when another client answered).

use super::gcode_store::{ConsoleKind, ConsoleLine};

/// What the client sends when the person closes the dialog: the same line Mainsail sends for its cross
/// (`closePrompt` in `TheMacroPrompt.vue`), so that the macro and the other clients learn about it.
pub const CLOSE_GCODE: &str = r#"RESPOND type="command" msg="action:prompt_end""#;

const PREFIX: &str = "action:prompt_";

/// The colour name of a button. Mainsail takes the theme colours of its UI toolkit (Vuetify: primary, secondary,
/// info, warning, error; Klipper's documentation names the same) and also any CSS colour for a `prompt_button`;
/// anything but the five names is left to the default look here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptStyle {
    Primary,
    Secondary,
    Info,
    Warning,
    Error,
}

impl PromptStyle {
    fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "primary" => Some(Self::Primary),
            "secondary" => Some(Self::Secondary),
            "info" => Some(Self::Info),
            "warning" => Some(Self::Warning),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptButton {
    pub label: String,
    /// What the button sends; the label if the macro gave none (`MacroPromptButton.vue`: `splits[1] ?? text`).
    pub gcode: String,
    pub style: Option<PromptStyle>,
}

impl PromptButton {
    /// `label|gcode|style`; Mainsail splits at `|` the same way. An empty gcode counts as missing: sending nothing
    /// would do nothing at all.
    fn parse(text: &str) -> Self {
        let mut parts = text.split('|');
        let label = parts.next().unwrap_or_default().trim().to_owned();
        let gcode = parts.next().map(str::trim).filter(|gcode| !gcode.is_empty()).unwrap_or(&label).to_owned();
        let style = parts.next().and_then(PromptStyle::parse);
        Self { label, gcode, style }
    }
}

/// A line of the dialog body, in the order the macro sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptItem {
    Text(String),
    /// Buttons that belong in one row: a `prompt_button_group_start`/`_end` pair, or a lone `prompt_button`.
    Buttons(Vec<PromptButton>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prompt {
    pub title: String,
    pub items: Vec<PromptItem>,
    pub footer: Vec<PromptButton>,
}

/// A prompt being built, with the button group that is still open.
#[derive(Debug, Clone, Default)]
struct Draft {
    prompt: Prompt,
    open_group: Option<Vec<PromptButton>>,
}

impl Draft {
    fn add_button(&mut self, button: PromptButton) {
        match &mut self.open_group {
            Some(group) => group.push(button),
            None => self.prompt.items.push(PromptItem::Buttons(vec![button])),
        }
    }

    /// The prompt as `prompt_show` found it. A group that never ended is no group in Mainsail: its buttons are
    /// shown one to a row, and the text it swallowed stays out.
    fn shown(&self) -> Prompt {
        let mut prompt = self.prompt.clone();
        for button in self.open_group.iter().flatten() {
            prompt.items.push(PromptItem::Buttons(vec![button.clone()]));
        }
        prompt
    }
}

#[derive(Debug, Clone, Default)]
pub struct PromptTracker {
    draft: Option<Draft>,
    shown: Option<Prompt>,
}

impl PromptTracker {
    /// The prompt to show, or none.
    pub fn current(&self) -> Option<&Prompt> {
        self.shown.as_ref()
    }

    /// Takes the console history, as it is after a (re)connect: what an open prompt looked like. Mainsail scans its
    /// loaded lines back to the last `prompt_begin` or `prompt_end` (`macroPromptEvents`), which gives the same as
    /// reading all lines in order.
    pub fn from_history(lines: &[ConsoleLine]) -> Self {
        let mut tracker = Self::default();
        for line in lines {
            tracker.feed(line);
        }
        tracker
    }

    /// Takes the console history unless a prompt is being followed live: the history holds only the last lines, and
    /// a long wait with many reports (temperatures while heating) pushes the prompt out of it. Returns whether the
    /// prompt to show changed.
    pub fn take_history(&mut self, lines: &[ConsoleLine]) -> bool {
        if self.draft.is_some() || self.shown.is_some() {
            return false;
        }
        *self = Self::from_history(lines);
        self.shown.is_some()
    }

    /// Forgets everything: the printer is gone, and so is what it asked. Returns whether a prompt was shown.
    pub fn reset(&mut self) -> bool {
        let was_shown = self.shown.is_some();
        *self = Self::default();
        was_shown
    }

    /// Reads one console line. Returns whether the prompt to show changed.
    pub fn feed(&mut self, line: &ConsoleLine) -> bool {
        let Some((kind, text)) = parse_line(line) else { return false };
        let before = self.shown.clone();
        match kind {
            "begin" => {
                self.draft =
                    Some(Draft { prompt: Prompt { title: text.to_owned(), ..Prompt::default() }, open_group: None });
                self.shown = None;
            }
            "end" => self.close(),
            "show" => self.shown = self.draft.as_ref().map(Draft::shown),
            _ => {
                if let Some(draft) = &mut self.draft {
                    add_content(draft, kind, text);
                }
            }
        }
        self.shown != before
    }

    /// The person closed the dialog (the caller sends [`CLOSE_GCODE`]): what the macro still sends of this prompt stays
    /// hidden, as in Mainsail (`internalCloseCommand`). Klipper may be busy and answer late.
    pub fn close(&mut self) {
        self.draft = None;
        self.shown = None;
    }
}

fn add_content(draft: &mut Draft, kind: &str, text: &str) {
    match kind {
        // Text inside a group is dropped, as in Mainsail (the group keeps its buttons only).
        "text" if draft.open_group.is_none() => draft.prompt.items.push(PromptItem::Text(text.to_owned())),
        "button" => draft.add_button(PromptButton::parse(text)),
        "button_group_start" => {
            draft.open_group.get_or_insert_with(Vec::new);
        }
        "button_group_end" => {
            if let Some(group) = draft.open_group.take().filter(|group| !group.is_empty()) {
                draft.prompt.items.push(PromptItem::Buttons(group));
            }
        }
        "footer_button" => draft.prompt.footer.push(PromptButton::parse(text)),
        _ => {}
    }
}

/// The part after `action:prompt_` and the rest of the line, for a response that is a prompt line.
fn parse_line(line: &ConsoleLine) -> Option<(&str, &str)> {
    if line.kind != ConsoleKind::Response {
        return None;
    }
    let rest = line.message.strip_prefix("//")?.trim_start().strip_prefix(PREFIX)?;
    let (kind, text) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    Some((kind, text.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(message: &str) -> ConsoleLine {
        ConsoleLine { kind: ConsoleKind::Response, message: format!("// action:prompt_{message}"), time: 0.0 }
    }

    fn tracker_after(lines: &[&str]) -> PromptTracker {
        let mut tracker = PromptTracker::default();
        for line in lines {
            tracker.feed(&response(line));
        }
        tracker
    }

    fn button(label: &str, gcode: &str, style: Option<PromptStyle>) -> PromptButton {
        PromptButton { label: label.to_owned(), gcode: gcode.to_owned(), style }
    }

    #[test]
    fn begin_text_buttons_show_make_a_prompt() {
        let tracker = tracker_after(&[
            "begin Is the bed empty?",
            "text Anything on it gets crushed.",
            "footer_button NO|BED_CLEAR CANCEL=1|secondary",
            "footer_button YES, EMPTY|BED_CLEAR|primary",
            "show",
        ]);
        let prompt = tracker.current().expect("open prompt");
        assert_eq!(prompt.title, "Is the bed empty?");
        assert_eq!(prompt.items, [PromptItem::Text("Anything on it gets crushed.".into())]);
        assert_eq!(
            prompt.footer,
            [
                button("NO", "BED_CLEAR CANCEL=1", Some(PromptStyle::Secondary)),
                button("YES, EMPTY", "BED_CLEAR", Some(PromptStyle::Primary)),
            ]
        );
    }

    #[test]
    fn nothing_is_open_before_show() {
        let tracker = tracker_after(&["begin Title", "text Hello"]);
        assert!(tracker.current().is_none());
    }

    #[test]
    fn gcode_defaults_to_the_label() {
        let tracker = tracker_after(&["begin T", "button Resume", "button Empty gcode||info", "show"]);
        let items = &tracker.current().unwrap().items;
        assert_eq!(items[0], PromptItem::Buttons(vec![button("Resume", "Resume", None)]));
        assert_eq!(items[1], PromptItem::Buttons(vec![button("Empty gcode", "Empty gcode", Some(PromptStyle::Info))]));
    }

    #[test]
    fn all_styles_and_an_unknown_one() {
        let tracker = tracker_after(&[
            "begin T",
            "button a|A|primary",
            "button b|B|secondary",
            "button c|C|info",
            "button d|D|warning",
            "button e|E|error",
            "button f|F|#ff8800",
            "button g|G| Error ",
            "show",
        ]);
        let styles: Vec<Option<PromptStyle>> = tracker
            .current()
            .unwrap()
            .items
            .iter()
            .map(|item| match item {
                PromptItem::Buttons(buttons) => buttons[0].style,
                PromptItem::Text(_) => unreachable!(),
            })
            .collect();
        assert_eq!(
            styles,
            [
                Some(PromptStyle::Primary),
                Some(PromptStyle::Secondary),
                Some(PromptStyle::Info),
                Some(PromptStyle::Warning),
                Some(PromptStyle::Error),
                None,
                Some(PromptStyle::Error),
            ]
        );
    }

    #[test]
    fn group_buttons_share_a_row() {
        let tracker = tracker_after(&[
            "begin T",
            "button_group_start",
            "button A|A",
            "button B|B",
            "button_group_end",
            "button C|C",
            "show",
        ]);
        assert_eq!(
            tracker.current().unwrap().items,
            [
                PromptItem::Buttons(vec![button("A", "A", None), button("B", "B", None)]),
                PromptItem::Buttons(vec![button("C", "C", None)]),
            ]
        );
    }

    #[test]
    fn a_group_that_never_ends_shows_its_buttons_one_to_a_row() {
        let tracker = tracker_after(&["begin T", "button_group_start", "button A", "button B", "show"]);
        assert_eq!(
            tracker.current().unwrap().items,
            [PromptItem::Buttons(vec![button("A", "A", None)]), PromptItem::Buttons(vec![button("B", "B", None)])]
        );
    }

    #[test]
    fn end_closes() {
        let mut tracker = tracker_after(&["begin T", "show"]);
        assert!(tracker.current().is_some());
        assert!(tracker.feed(&response("end")));
        assert!(tracker.current().is_none());
    }

    #[test]
    fn end_without_begin_changes_nothing() {
        let mut tracker = PromptTracker::default();
        assert!(!tracker.feed(&response("end")));
        assert!(tracker.current().is_none());
    }

    #[test]
    fn lines_before_a_begin_are_ignored() {
        let tracker = tracker_after(&["text stray", "button X", "begin T", "show"]);
        assert!(tracker.current().unwrap().items.is_empty());
    }

    #[test]
    fn a_second_begin_replaces_the_open_prompt() {
        let mut tracker = tracker_after(&["begin First", "text one", "show"]);
        assert!(tracker.feed(&response("begin Second")));
        assert!(tracker.current().is_none(), "hidden until the new prompt is shown");
        tracker.feed(&response("text two"));
        tracker.feed(&response("show"));
        let prompt = tracker.current().unwrap();
        assert_eq!(prompt.title, "Second");
        assert_eq!(prompt.items, [PromptItem::Text("two".into())]);
    }

    #[test]
    fn content_after_the_last_show_waits_for_the_next_show() {
        let mut tracker = tracker_after(&["begin T", "text one", "show", "text two"]);
        assert_eq!(tracker.current().unwrap().items.len(), 1);
        assert!(tracker.feed(&response("show")));
        assert_eq!(tracker.current().unwrap().items.len(), 2);
    }

    #[test]
    fn closing_by_hand_hides_the_rest_of_that_prompt() {
        let mut tracker = tracker_after(&["begin T", "show"]);
        tracker.close();
        tracker.feed(&response("text late"));
        tracker.feed(&response("show"));
        assert!(tracker.current().is_none());
        tracker.feed(&response("begin Next"));
        tracker.feed(&response("show"));
        assert_eq!(tracker.current().unwrap().title, "Next");
    }

    #[test]
    fn other_console_lines_are_ignored() {
        let mut tracker = tracker_after(&["begin T", "show"]);
        let command = ConsoleLine { kind: ConsoleKind::Command, message: "// action:prompt_end".into(), time: 0.0 };
        assert!(!tracker.feed(&command));
        assert!(!tracker.feed(&ConsoleLine {
            kind: ConsoleKind::Response,
            message: "// action:notification x".into(),
            time: 0.0
        }));
        assert!(!tracker.feed(&ConsoleLine { kind: ConsoleKind::Response, message: "ok".into(), time: 0.0 }));
        assert!(tracker.current().is_some());
    }

    #[test]
    fn history_with_an_open_prompt_after_a_closed_one() {
        let lines: Vec<ConsoleLine> =
            ["begin Old", "show", "end", "begin Open", "text still asking", "button Go|GO|primary", "show"]
                .into_iter()
                .map(response)
                .collect();
        let tracker = PromptTracker::from_history(&lines);
        assert_eq!(tracker.current().unwrap().title, "Open");
    }

    #[test]
    fn history_does_not_replace_a_prompt_followed_live() {
        let mut tracker = tracker_after(&["begin Live", "show"]);
        let history: Vec<ConsoleLine> = ["begin Old", "show", "end"].into_iter().map(response).collect();
        assert!(!tracker.take_history(&history));
        assert_eq!(tracker.current().unwrap().title, "Live");
    }

    #[test]
    fn history_opens_a_prompt_when_none_is_followed() {
        let mut tracker = PromptTracker::default();
        let history: Vec<ConsoleLine> = ["begin Open", "show"].into_iter().map(response).collect();
        assert!(tracker.take_history(&history));
        assert_eq!(tracker.current().unwrap().title, "Open");
    }

    #[test]
    fn reset_forgets_the_prompt() {
        let mut tracker = tracker_after(&["begin T", "show"]);
        assert!(tracker.reset());
        assert!(tracker.current().is_none());
        tracker.feed(&response("text late"));
        tracker.feed(&response("show"));
        assert!(tracker.current().is_none());
    }

    #[test]
    fn history_whose_last_prompt_ended_has_none() {
        let lines: Vec<ConsoleLine> = ["begin Old", "show", "end"].into_iter().map(response).collect();
        assert!(PromptTracker::from_history(&lines).current().is_none());
    }

    #[test]
    fn history_with_begin_but_no_show_has_none() {
        let lines: Vec<ConsoleLine> = ["begin Old", "show", "end", "begin New"].into_iter().map(response).collect();
        assert!(PromptTracker::from_history(&lines).current().is_none());
    }

    #[test]
    fn title_and_text_keep_inner_spaces_and_pipes() {
        let tracker = tracker_after(&["begin  A  B ", "text a | b", "show"]);
        let prompt = tracker.current().unwrap();
        assert_eq!(prompt.title, "A  B");
        assert_eq!(prompt.items, [PromptItem::Text("a | b".into())]);
    }

    #[test]
    fn tolerates_a_missing_space_after_the_slashes() {
        let mut tracker = PromptTracker::default();
        tracker.feed(&ConsoleLine {
            kind: ConsoleKind::Response,
            message: "//action:prompt_begin T".into(),
            time: 0.0,
        });
        tracker.feed(&ConsoleLine { kind: ConsoleKind::Response, message: "// action:prompt_show".into(), time: 0.0 });
        assert_eq!(tracker.current().unwrap().title, "T");
    }
}
