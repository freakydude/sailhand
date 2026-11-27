//! Runs of macros started from this display, and what Klipper answered while each ran.
//!
//! Moonraker answers `printer.gcode.script` only when Klipper has finished the script, and Klipper runs G-code one
//! command after the other. So the responses between sending a macro and its answer came while it ran. Klipper does not
//! say which command wrote a line: another client's command or a plugin may have written it, so the lines are "during
//! the run", not "of the macro". Two runs at the same time both get the lines of their overlap.
//!
//! Only the last run of each macro is kept, for as long as the app talks to the same printer.

use std::collections::VecDeque;

use crate::sources::moonraker_server::gcode_store::{ConsoleKind, ConsoleLine};

/// A long run (a heat soak) keeps its newest lines; the console keeps the same number.
const LINE_LIMIT: usize = 300;

#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    Running,
    Done,
    /// Klipper refused or aborted the script, or the connection ended before the answer; the reason.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MacroRun {
    /// The id the service gave the run when it sent it.
    pub id: u64,
    /// The macro, as its name came in the script.
    pub name: String,
    /// The script as sent, with its parameters (`PARK Z=20`).
    pub script: String,
    /// Unix time in seconds when it was sent.
    pub started: f64,
    /// Unix time in seconds of the answer; `None` while it runs.
    pub ended: Option<f64>,
    pub outcome: RunOutcome,
    /// The responses while it ran, oldest first; without temperature reports and the protocol of macro prompts.
    pub lines: VecDeque<ConsoleLine>,
}

impl MacroRun {
    pub fn is_running(&self) -> bool {
        self.outcome == RunOutcome::Running
    }

    /// Seconds from sending to the answer, once there is one.
    pub fn duration(&self) -> Option<f64> {
        self.ended.map(|ended| (ended - self.started).max(0.0))
    }
}

#[derive(Debug, Default)]
pub struct MacroRuns {
    /// The last run of each macro, in the order they were started.
    runs: Vec<MacroRun>,
}

impl MacroRuns {
    /// A macro was sent: it is the last run of its macro now.
    pub fn start(&mut self, id: u64, script: &str, now: f64) {
        let script = script.trim();
        let name = macro_name(script).to_owned();
        self.runs.retain(|run| !run.name.eq_ignore_ascii_case(&name));
        self.runs.push(MacroRun {
            id,
            name,
            script: script.to_owned(),
            started: now,
            ended: None,
            outcome: RunOutcome::Running,
            lines: VecDeque::new(),
        });
    }

    /// A line arrived live: every run that is still going gets it. Returns the names of the macros that got it.
    pub fn add_line(&mut self, line: &ConsoleLine) -> Vec<&str> {
        if !belongs_to_a_run(line) {
            return Vec::new();
        }
        let mut names = Vec::new();
        for run in self.runs.iter_mut().filter(|run| run.is_running()) {
            if run.lines.len() >= LINE_LIMIT {
                run.lines.pop_front();
            }
            run.lines.push_back(line.clone());
            names.push(run.name.as_str());
        }
        names
    }

    /// The answer of a run came: `None` when Klipper ran it, else the reason it failed. Returns its macro's name,
    /// unless a newer run of the same macro replaced it already.
    pub fn finish(&mut self, id: u64, error: Option<String>, now: f64) -> Option<&str> {
        let run = self.runs.iter_mut().find(|run| run.id == id)?;
        run.ended = Some(now);
        run.outcome = error.map_or(RunOutcome::Done, RunOutcome::Failed);
        Some(run.name.as_str())
    }

    /// Whether a run that is still going belongs to this macro; the line just added reached it then.
    pub fn is_running(&self, name: &str) -> bool {
        self.last(name).is_some_and(MacroRun::is_running)
    }

    /// The last run of a macro from this display.
    pub fn last(&self, name: &str) -> Option<&MacroRun> {
        self.runs.iter().find(|run| run.name.eq_ignore_ascii_case(name))
    }

    /// Another printer: its macros are others.
    pub fn clear(&mut self) {
        self.runs.clear();
    }
}

/// The macro a script calls: its first word (`PARK Z=20` → `PARK`).
pub fn macro_name(script: &str) -> &str {
    script.split_whitespace().next().unwrap_or_default()
}

/// Responses only: the commands are the person's own, temperature reports would bury the rest during a heat-up, and
/// the lines of a macro prompt are the prompt dialog's.
fn belongs_to_a_run(line: &ConsoleLine) -> bool {
    line.kind == ConsoleKind::Response && !line.is_temperature_report() && !line.is_host_action()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(message: &str) -> ConsoleLine {
        ConsoleLine { kind: ConsoleKind::Response, message: message.to_owned(), time: 0.0 }
    }

    fn texts(run: &MacroRun) -> Vec<&str> {
        run.lines.iter().map(|line| line.message.as_str()).collect()
    }

    #[test]
    fn lines_between_sending_and_answer_belong_to_the_run() {
        let mut runs = MacroRuns::default();
        runs.add_line(&response("// before"));
        runs.start(1, "HEAT_SOAK TEMP=60", 100.0);
        assert_eq!(runs.add_line(&response("// soaking")), ["HEAT_SOAK"]);
        assert_eq!(runs.finish(1, None, 160.0), Some("HEAT_SOAK"));
        assert!(runs.add_line(&response("// after")).is_empty());

        let run = runs.last("heat_soak").unwrap();
        assert_eq!(run.script, "HEAT_SOAK TEMP=60");
        assert_eq!(texts(run), ["// soaking"]);
        assert_eq!(run.outcome, RunOutcome::Done);
        assert_eq!(run.duration(), Some(60.0));
    }

    #[test]
    fn overlapping_runs_both_get_the_lines_of_the_overlap() {
        let mut runs = MacroRuns::default();
        runs.start(1, "PARK", 0.0);
        runs.start(2, "WIPE", 1.0);
        assert_eq!(runs.add_line(&response("// both")), ["PARK", "WIPE"]);
        runs.finish(1, None, 2.0);
        runs.add_line(&response("// wipe only"));

        assert_eq!(texts(runs.last("PARK").unwrap()), ["// both"]);
        assert_eq!(texts(runs.last("WIPE").unwrap()), ["// both", "// wipe only"]);
    }

    #[test]
    fn a_new_run_replaces_the_last_one_of_its_macro() {
        let mut runs = MacroRuns::default();
        runs.start(1, "PARK Z=10", 0.0);
        runs.add_line(&response("// first"));
        runs.start(2, "park Z=20", 1.0);

        let run = runs.last("PARK").unwrap();
        assert_eq!((run.id, run.script.as_str()), (2, "park Z=20"));
        assert!(run.lines.is_empty());
        // The answer of the replaced run changes nothing.
        assert_eq!(runs.finish(1, None, 2.0), None);
        assert!(runs.is_running("PARK"));
    }

    #[test]
    fn a_failed_run_keeps_the_reason() {
        let mut runs = MacroRuns::default();
        runs.start(1, "PARK", 0.0);
        runs.finish(1, Some("Must home axis first".into()), 0.5);
        assert_eq!(runs.last("PARK").unwrap().outcome, RunOutcome::Failed("Must home axis first".into()));
    }

    #[test]
    fn commands_temperature_reports_and_prompt_lines_stay_out() {
        let mut runs = MacroRuns::default();
        runs.start(1, "HEAT_SOAK", 0.0);
        let command = ConsoleLine { kind: ConsoleKind::Command, message: "M105".into(), time: 0.0 };
        assert!(runs.add_line(&command).is_empty());
        assert!(runs.add_line(&response("B:59.8 /60.0 T0:24.1 /0.0")).is_empty());
        assert!(runs.add_line(&response("// action:prompt_begin Is the bed empty?")).is_empty());
        assert!(runs.last("HEAT_SOAK").unwrap().lines.is_empty());
    }

    #[test]
    fn a_long_run_keeps_its_newest_lines() {
        let mut runs = MacroRuns::default();
        runs.start(1, "HEAT_SOAK", 0.0);
        for index in 0..=LINE_LIMIT {
            runs.add_line(&response(&format!("// {index}")));
        }
        let run = runs.last("HEAT_SOAK").unwrap();
        assert_eq!(run.lines.len(), LINE_LIMIT);
        assert_eq!(run.lines.front().unwrap().message, "// 1");
    }
}
