//! Macro prompts of supported projects whose content Sailhand already shows natively, so the dialog would only say it
//! twice. The prompt protocol itself ([`crate::sources::moonraker_server::prompt`]) knows nothing of projects; each
//! rule here is self-contained with its source and tests, and is removed again (function plus its entry in
//! [`RULES`]) once the project offers the same directly.

use crate::sources::PrinterStatus;
use crate::sources::moonraker_server::prompt::Prompt;

/// What becomes of a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Coverage {
    /// Show it as a dialog.
    Show,
    /// Leave the dialog out; `carry` holds the lines that are said nowhere else, for the place that covers it.
    Hide { carry: Vec<String> },
}

impl Coverage {
    pub fn is_hidden(&self) -> bool {
        matches!(self, Self::Hide { .. })
    }

    /// The lines to show elsewhere; none for a prompt that is shown.
    pub fn carried(self) -> Vec<String> {
        match self {
            Self::Show => Vec::new(),
            Self::Hide { carry } => carry,
        }
    }
}

/// A rule for one project's prompt: `None` if the prompt is not its own.
type Rule = fn(&Prompt, &PrinterStatus) -> Option<Coverage>;

const RULES: &[Rule] = &[happy_hare::pause_notice];

/// Whether the prompt is shown as a dialog or covered by Sailhand's own view.
pub fn coverage(prompt: &Prompt, status: &PrinterStatus) -> Coverage {
    RULES.iter().find_map(|rule| rule(prompt, status)).unwrap_or(Coverage::Show)
}

mod happy_hare {
    //! Happy Hare's pause dialog, `MmuErrorDialog.show_error` in `extras/mmu/mmu_dialog.py` (`main` and
    //! `development`, 2026-11), called from `handle_mmu_error` in `mmu_controller.py` (L2663ff.). Text lines in this
    //! order: [the headline "Print is still paused", only for a repeated error] [the context] [the reason as one
    //! paragraph] [lines of `MMU_ERROR_DIALOG MSG=`] ["Fix: <pause_hint>", maybe led by "UNLOCK to restore heating,
    //! then", else `DIALOG_RESUME_HINT` without `MSG=` lines]. Footer: `ABORT|CANCEL_PRINT|error` (with `ABORT=1`),
    //! `RESUME|RESUME|warning`, `UNLOCK|MMU_UNLOCK|primary`; `development` puts the buttons of
    //! `MMU_ERROR_DIALOG BUTTONS=` before them. Sailhand shows the reason and hint in the pause banner (from
    //! `reason_for_pause`/`pause_hint`), Resume and Cancel on the print page and Unlock in the MMU strip.

    use super::Coverage;
    use crate::sources::PrinterStatus;
    use crate::sources::moonraker_server::prompt::{Prompt, PromptItem};

    const TITLE: &str = "Happy Hare Pause Notice";
    const STANDARD_GCODES: [&str; 3] = ["CANCEL_PRINT", "RESUME", "MMU_UNLOCK"];
    const STILL_PAUSED: &str = "Print is still paused";
    const RESUME_HINT: &str =
        "After fixing, press RESUME to continue printing (UNLOCK restores the nozzle temperature first)";
    const FIX_PREFIX: &str = "Fix:";

    /// Hidden while only the standard buttons are in it; the lines not stated by the reason go to the banner.
    pub(super) fn pause_notice(prompt: &Prompt, status: &PrinterStatus) -> Option<Coverage> {
        let mmu = status.happy_hare()?;
        if prompt.title != TITLE {
            return None;
        }
        if !only_standard_buttons(prompt) {
            return Some(Coverage::Show);
        }
        let reason = normalized(&mmu.reason_for_pause);
        let carry = prompt
            .items
            .iter()
            .filter_map(|item| match item {
                PromptItem::Text(text) => Some(text.trim()),
                PromptItem::Buttons(_) => None,
            })
            .filter(|line| is_extra_line(line, &reason))
            .map(str::to_owned)
            .collect();
        Some(Coverage::Hide { carry })
    }

    fn only_standard_buttons(prompt: &Prompt) -> bool {
        let body_buttons = prompt.items.iter().any(|item| matches!(item, PromptItem::Buttons(_)));
        !body_buttons && prompt.footer.iter().all(|button| STANDARD_GCODES.contains(&button.gcode.as_str()))
    }

    /// A line of `MSG=`, or one the reason does not hold (a new reason after a repeated error).
    fn is_extra_line(line: &str, reason: &str) -> bool {
        let line_key = normalized(line);
        !line_key.is_empty()
            && !line.starts_with(FIX_PREFIX)
            && line != RESUME_HINT
            && line != STILL_PAUSED
            && !reason.contains(&line_key)
    }

    /// Letters and digits only, lower case: Happy Hare flattens whitespace and adds full stops for the dialog.
    fn normalized(text: &str) -> String {
        text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::happy_hare::MmuStatus;
    use crate::sources::moonraker_server::prompt::{PromptButton, PromptItem, PromptStyle};

    const REASON: &str = "Runout at gate 1\nwhile loading";

    fn status_with_happy_hare() -> PrinterStatus {
        PrinterStatus {
            mmu: Some(MmuStatus {
                version: "4.1.0".into(),
                enabled: true,
                reason_for_pause: REASON.into(),
                ..MmuStatus::default()
            }),
            ..PrinterStatus::default()
        }
    }

    fn button(label: &str, gcode: &str, style: PromptStyle) -> PromptButton {
        PromptButton { label: label.into(), gcode: gcode.into(), style: Some(style) }
    }

    /// The notice as `show_error` builds it, with the abort button and one `MSG=` line.
    fn notice(lines: &[&str]) -> Prompt {
        Prompt {
            title: "Happy Hare Pause Notice".into(),
            items: lines.iter().map(|line| PromptItem::Text((*line).into())).collect(),
            footer: vec![
                button("ABORT", "CANCEL_PRINT", PromptStyle::Error),
                button("RESUME", "RESUME", PromptStyle::Warning),
                button("UNLOCK", "MMU_UNLOCK", PromptStyle::Primary),
            ],
        }
    }

    #[test]
    fn the_notice_is_hidden_and_carries_only_the_msg_lines() {
        let prompt = notice(&[
            "Print is still paused",
            "Runout at gate 1. While loading.",
            "Check the spool holder",
            "Fix: UNLOCK to restore heating, then reload gate 1",
        ]);

        let coverage = coverage(&prompt, &status_with_happy_hare());

        assert_eq!(coverage, Coverage::Hide { carry: vec!["Check the spool holder".into()] });
    }

    #[test]
    fn the_standard_resume_hint_is_not_carried() {
        let prompt = notice(&[
            "Runout at gate 1 while loading",
            "After fixing, press RESUME to continue printing (UNLOCK restores the nozzle temperature first)",
        ]);

        assert_eq!(coverage(&prompt, &status_with_happy_hare()), Coverage::Hide { carry: vec![] });
    }

    #[test]
    fn a_new_reason_after_a_repeated_error_is_carried() {
        let prompt = notice(&["Print is still paused", "Gate 2 is empty."]);

        assert_eq!(coverage(&prompt, &status_with_happy_hare()).carried(), ["Gate 2 is empty."]);
    }

    #[test]
    fn own_buttons_show_the_dialog() {
        let mut prompt = notice(&["Runout at gate 1"]);
        prompt.footer.insert(0, button("Purge", "PURGE_TOOL", PromptStyle::Secondary));

        assert_eq!(coverage(&prompt, &status_with_happy_hare()), Coverage::Show);
    }

    #[test]
    fn without_happy_hare_the_notice_is_shown() {
        let prompt = notice(&["Runout at gate 1"]);

        assert_eq!(coverage(&prompt, &PrinterStatus::default()), Coverage::Show);
    }

    #[test]
    fn other_prompts_are_shown() {
        let mut prompt = notice(&["Runout at gate 1"]);
        prompt.title = "Filament change".into();

        assert_eq!(coverage(&prompt, &status_with_happy_hare()), Coverage::Show);
    }
}
