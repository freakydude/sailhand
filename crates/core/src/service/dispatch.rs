//! Carrying out commands: an intent becomes concrete calls that go to Moonraker in the order they were sent.

use super::session::Session;
use super::trail;
use super::{Event, LONG_COMMAND, NEXT_COMMAND_ID};
use crate::commands::{Command, CommandError};
use crate::moonraker::{Call, CallError, PendingCall};
use crate::sources::PrinterStatus;
use crate::sources::moonraker_server::gcode_store::{ConsoleKind, ConsoleLine};
use crate::sources::moonraker_server::wled;
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

impl Session<'_> {
    /// Carries out a command; an intent first becomes the commands this printer needs.
    pub(super) fn execute(&mut self, command: Command) {
        log::debug!("command {command:?}");
        let status = self.current_status();
        if command.moves_machine() && status.print_moving() {
            self.refuse(command, CommandError::PrintMovesMachine);
            return;
        }
        if command.loses_print_position() && status.print_running() {
            self.refuse(command, CommandError::PrintNeedsPosition);
            return;
        }
        let commands = match command.clone().resolve(&self.sources, &status) {
            Ok(commands) => commands,
            Err(error) => {
                self.refuse(command, error);
                return;
            }
        };
        // Asked of the intent: resolved, a spool assignment is G-code like any other.
        let spools_change = command.changes_spools();
        let mut targets_changed = false;
        for command in commands {
            targets_changed |= self.dispatch(command, &status, spools_change);
        }
        if targets_changed {
            self.publish();
        }
    }

    /// Sends one concrete command; true if it set a heater target, which the snapshot shows at once.
    fn dispatch(&mut self, command: Command, status: &PrinterStatus, spools_change: bool) -> bool {
        if command.interrupts_print() && status.print_running() {
            self.refuse(command, CommandError::PrintRunning);
            return false;
        }
        let Some(call) = command.to_call() else { return false };
        let method = call.method;
        log::debug!("call {method} {}", call.params.as_ref().map(trail::brief).unwrap_or_default());
        // Every G-code script goes to the console, as Moonraker stores it in the console history: typed commands,
        // macros and the scripts behind the buttons alike.
        if let Some(script) = gcode_script(&call) {
            self.outputs.event(Event::ConsoleLine(console_line(ConsoleKind::Command, script)));
        }
        let macro_run = matches!(command, Command::RunMacro { .. }).then(|| self.start_macro_run(&call));
        let target_set = match &command {
            Command::SetHeaterTarget { heater, target } => {
                self.pending_targets.request(heater, *target, Instant::now());
                true
            }
            Command::Move { planned } => {
                self.pending_moves.request(planned, Instant::now());
                false
            }
            _ => false,
        };
        let spools_change = spools_change || command.changes_spools();
        // Busy from the start: Moonraker's own `busy` and notifications only follow once it has begun.
        let update_manager_call = matches!(command, Command::RefreshUpdates | Command::Upgrade { .. });
        if update_manager_call {
            self.outputs.event(Event::UpdateManagerBusy(true));
        }
        let wled_answers = matches!(command, Command::SetWledStrip { .. }).then(|| self.wled_answers.clone());
        let running = Running { command: command.clone(), script_line: first_script_line(&call) };
        let spool_events = self.outputs.events.clone();
        let running_events = self.outputs.events.clone();
        let events = self.outputs.events.clone();
        let failed = move |error: CallError| {
            let error = match error {
                CallError::Rpc(rpc) => CommandError::Rejected(rpc.message),
                CallError::Closed => CommandError::ConnectionClosed,
            };
            log::info!("{method} failed: {error}");
            let _ = events.send(Event::CommandFailed { command, error });
        };
        // Sent here, in the order of the commands; a task per command could overtake an earlier one.
        match self.connection.start_call(call.method, call.params) {
            Ok(pending) => {
                let sent = Instant::now();
                tokio::spawn(async move {
                    let answer = answer_or_report_running(pending, running, &running_events).await;
                    if answer.is_ok() {
                        log::debug!("{method} answered after {} ms", sent.elapsed().as_millis());
                    }
                    if update_manager_call {
                        let _ = running_events.send(Event::UpdateManagerBusy(false));
                    }
                    if let Some(run) = macro_run {
                        run.answered(answer.as_ref().err(), &running_events);
                    }
                    match answer {
                        Ok(_) if spools_change => {
                            let _ = spool_events.send(Event::SpoolmanChanged);
                        }
                        Ok(answer) => {
                            if let (Some(sender), Some(strips)) = (wled_answers, answer.as_object()) {
                                let _ = sender.send(wled::parse_strips(strips));
                            }
                        }
                        Err(error) => failed(error),
                    }
                });
            }
            Err(error) => {
                if let Some(run) = macro_run {
                    run.answered(Some(&error), &self.outputs.events);
                }
                failed(error);
            }
        }
        target_set
    }

    /// Announces a macro run before its answer can come; the answer goes back through the session loop.
    fn start_macro_run(&self, call: &Call) -> MacroRunAnswer {
        let id = NEXT_COMMAND_ID.fetch_add(1, Ordering::Relaxed);
        let script = gcode_script(call).unwrap_or_default().to_owned();
        self.outputs.event(Event::MacroRunStarted { id, script, time: unix_now() });
        MacroRunAnswer { id, answers: self.run_answers.clone() }
    }

    /// Reports a command that is not carried out, and why.
    fn refuse(&self, command: Command, error: CommandError) {
        log::debug!("command {command:?} refused: {error}");
        self.outputs.event(Event::CommandFailed { command, error });
    }
}

/// Waits for the answer of a call; if it takes longer than `LONG_COMMAND`, reports the command as running until then.
async fn answer_or_report_running(
    pending: PendingCall,
    running: Running,
    events: &mpsc::UnboundedSender<Event>,
) -> Result<Value, CallError> {
    let response = pending.response();
    tokio::pin!(response);
    tokio::select! {
        answer = &mut response => answer,
        () = tokio::time::sleep(LONG_COMMAND) => {
            let id = NEXT_COMMAND_ID.fetch_add(1, Ordering::Relaxed);
            let _ = events.send(Event::CommandRunning { id, command: running.command, script_line: running.script_line });
            let answer = response.await;
            let _ = events.send(Event::CommandFinished { id });
            answer
        }
    }
}

/// A command waiting for its answer, as `Event::CommandRunning` reports it.
struct Running {
    command: Command,
    script_line: Option<String>,
}

/// The first line of the G-code script of a call, as the console shows it.
fn first_script_line(call: &Call) -> Option<String> {
    gcode_script(call).and_then(|script| script.lines().next()).map(str::to_owned)
}

/// The script of a call that runs G-code.
fn gcode_script(call: &Call) -> Option<&str> {
    if call.method != "printer.gcode.script" {
        return None;
    }
    call.params.as_ref().and_then(|params| params.get("script")).and_then(Value::as_str)
}

pub(super) fn console_line(kind: ConsoleKind, message: &str) -> ConsoleLine {
    ConsoleLine { kind, message: message.to_owned(), time: unix_now() }
}

fn unix_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

/// Where the answer of a macro run goes: back to the session loop, which ends the run after the responses before it.
pub(super) struct MacroRunAnswer {
    id: u64,
    answers: mpsc::UnboundedSender<RunAnswer>,
}

impl MacroRunAnswer {
    /// A closed connection sends no more responses, so its end needs no queue; nor does a session that is gone.
    fn answered(self, error: Option<&CallError>, events: &mpsc::UnboundedSender<Event>) {
        let closed = matches!(error, Some(CallError::Closed));
        let error = error.map(|error| match error {
            CallError::Rpc(rpc) => rpc.message.clone(),
            CallError::Closed => CommandError::ConnectionClosed.to_string(),
        });
        let answer = RunAnswer { id: self.id, error, time: unix_now() };
        let unsent = if closed { Some(answer) } else { self.answers.send(answer).err().map(|unsent| unsent.0) };
        if let Some(answer) = unsent {
            let _ = events.send(answer.into_event());
        }
    }
}

/// The answer of a macro run as the session loop gets it.
pub(super) struct RunAnswer {
    id: u64,
    error: Option<String>,
    time: f64,
}

impl RunAnswer {
    pub(super) fn into_event(self) -> Event {
        Event::MacroRunEnded { id: self.id, error: self.error, time: self.time }
    }
}
