//! The macro prompt dialog: the prompt the core follows in the console lines, as the rows and buttons of the
//! `PromptDialog` global. A prompt Sailhand covers natively (`covered_prompts`) stays out of the dialog.

use crate::{AppWindow, PromptButtonItem, PromptDialog, PromptRowItem, PromptStyle};
use sailhand_core::domain::covered_prompts::{self, Coverage};
use sailhand_core::sources::PrinterStatus;
use sailhand_core::sources::moonraker_server::gcode_store::ConsoleLine;
use sailhand_core::sources::moonraker_server::prompt::{self, Prompt, PromptButton, PromptItem, PromptTracker};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;

pub struct PromptView {
    window: slint::Weak<AppWindow>,
    tracker: RefCell<PromptTracker>,
}

impl PromptView {
    pub fn new(window: &AppWindow) -> Self {
        Self { window: window.as_weak(), tracker: RefCell::default() }
    }

    /// The history after a (re)connect: a prompt that is still open comes back. True if the prompt changed.
    pub fn show_history(&self, lines: &[ConsoleLine], status: &PrinterStatus) -> bool {
        let changed = self.tracker.borrow_mut().take_history(lines);
        if changed {
            self.show(status);
        }
        changed
    }

    /// A console line that arrived live. True if the prompt changed.
    pub fn push(&self, line: &ConsoleLine, status: &PrinterStatus) -> bool {
        let changed = self.tracker.borrow_mut().feed(line);
        if changed {
            self.show(status);
        }
        changed
    }

    /// A new printer state. Without Moonraker or Klipper nobody answers a prompt any more; otherwise whether a prompt
    /// is covered may have changed (Happy Hare detected after the prompt arrived).
    pub fn follow(&self, ready: bool, status: &PrinterStatus) {
        let reset = !ready && self.tracker.borrow_mut().reset();
        let shown = self.window.upgrade().is_some_and(|window| window.global::<PromptDialog>().get_open());
        if reset || shown != self.dialog_prompt(status).is_some() {
            self.show(status);
        }
    }

    /// The person closed the dialog: it goes away at once, as Klipper may be busy and answer late. The caller sends
    /// [`prompt::CLOSE_GCODE`].
    pub fn close(&self, status: &PrinterStatus) {
        self.tracker.borrow_mut().close();
        self.show(status);
    }

    /// The lines of a covered prompt that belong in Sailhand's own view (the pause banner).
    pub fn carried(&self, status: &PrinterStatus) -> Vec<String> {
        self.tracker
            .borrow()
            .current()
            .map(|prompt| covered_prompts::coverage(prompt, status).carried())
            .unwrap_or_default()
    }

    /// The prompt to show as a dialog: the open one unless it is covered.
    fn dialog_prompt(&self, status: &PrinterStatus) -> Option<Prompt> {
        let tracker = self.tracker.borrow();
        let prompt = tracker.current()?;
        (covered_prompts::coverage(prompt, status) == Coverage::Show).then(|| prompt.clone())
    }

    fn show(&self, status: &PrinterStatus) {
        let Some(window) = self.window.upgrade() else { return };
        let dialog = window.global::<PromptDialog>();
        match self.dialog_prompt(status) {
            Some(prompt) => {
                dialog.set_title(prompt.title.as_str().into());
                dialog.set_rows(ModelRc::new(VecModel::from(prompt_rows(&prompt))));
                dialog.set_footer(ModelRc::new(VecModel::from(button_items(&prompt.footer))));
                dialog.set_open(true);
            }
            None => dialog.set_open(false),
        }
    }
}

fn prompt_rows(prompt: &Prompt) -> Vec<PromptRowItem> {
    prompt
        .items
        .iter()
        .map(|item| match item {
            PromptItem::Text(text) => {
                PromptRowItem { text: text.as_str().into(), buttons: ModelRc::new(VecModel::default()) }
            }
            PromptItem::Buttons(buttons) => {
                PromptRowItem { text: Default::default(), buttons: ModelRc::new(VecModel::from(button_items(buttons))) }
            }
        })
        .collect()
}

fn button_items(buttons: &[PromptButton]) -> Vec<PromptButtonItem> {
    buttons
        .iter()
        .map(|button| PromptButtonItem {
            label: button.label.as_str().into(),
            gcode: button.gcode.as_str().into(),
            style: match button.style {
                None => PromptStyle::Plain,
                Some(prompt::PromptStyle::Primary) => PromptStyle::Primary,
                Some(prompt::PromptStyle::Secondary) => PromptStyle::Secondary,
                Some(prompt::PromptStyle::Info) => PromptStyle::Info,
                Some(prompt::PromptStyle::Warning) => PromptStyle::Warning,
                Some(prompt::PromptStyle::Error) => PromptStyle::Error,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    fn button(label: &str, style: Option<prompt::PromptStyle>) -> PromptButton {
        PromptButton { label: label.to_owned(), gcode: format!("{label}_GCODE"), style }
    }

    #[test]
    fn text_and_button_rows_keep_their_order() {
        let prompt = Prompt {
            title: "T".to_owned(),
            items: vec![
                PromptItem::Text("Hello".to_owned()),
                PromptItem::Buttons(vec![button("A", None), button("B", Some(prompt::PromptStyle::Error))]),
            ],
            footer: vec![],
        };
        let rows = prompt_rows(&prompt);
        assert_eq!(rows[0].text, "Hello");
        assert_eq!(rows[0].buttons.row_count(), 0);
        assert!(rows[1].text.is_empty());
        let buttons: Vec<PromptButtonItem> = rows[1].buttons.iter().collect();
        assert_eq!(buttons[0].label, "A");
        assert_eq!(buttons[0].gcode, "A_GCODE");
        assert_eq!(buttons[0].style, PromptStyle::Plain);
        assert_eq!(buttons[1].style, PromptStyle::Error);
    }

    #[test]
    fn every_style_maps_to_its_own() {
        use prompt::PromptStyle as Core;
        let styles = [Core::Primary, Core::Secondary, Core::Info, Core::Warning, Core::Error];
        let buttons: Vec<PromptButton> = styles.into_iter().map(|style| button("X", Some(style))).collect();
        let items = button_items(&buttons);
        assert_eq!(
            items.iter().map(|item| item.style).collect::<Vec<_>>(),
            [PromptStyle::Primary, PromptStyle::Secondary, PromptStyle::Info, PromptStyle::Warning, PromptStyle::Error]
        );
    }
}
