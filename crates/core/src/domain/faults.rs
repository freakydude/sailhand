//! Faults: states of the printer that report a problem and end by themselves when the state ends, such as Klipper in
//! shutdown or a paused print. Each fault names the place where it is dealt with; the app jumps there.

use crate::domain::changer;
use crate::domain::heads::{self, ChangerNote};
use crate::domain::pause::{self, Interruption};
use crate::sources::klipper::KlippyState;
use crate::{ConnectionState, Snapshot};

/// A state that reports a problem, as long as it lasts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// Moonraker does not answer (it answered before or never did).
    MoonrakerUnreachable,
    /// Klipper is in shutdown (`shutdown: true`) or could not start (error); `message` is its first line.
    Klipper { shutdown: bool, message: String },
    /// The print is paused or stopped with an error, with every fact a source states.
    Print(Interruption),
    /// AFC is in its error state.
    AfcError,
    /// The tool changer is in its error state; its text is only in the console.
    ChangerError,
    /// Moonraker is configured for Spoolman but Spoolman does not answer.
    SpoolmanOffline,
}

/// How much a fault asks of a person: something to look at, or something that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FaultLevel {
    Attention,
    Error,
}

/// The place where a person deals with an incident, in the words of the printer; the app maps it to a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// Moonraker and Klipper: the connection, Klipper's message and its restart.
    Connection,
    /// The print and its pause.
    Print,
    /// AFC's service functions, such as clearing the error.
    AfcService,
    /// The tool changer's service functions, such as recovering.
    ChangerService,
    /// The spool in use and Spoolman.
    Spools,
}

/// What stays the same over the life of a fault, so that a changing text does not start a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultKind {
    MoonrakerUnreachable,
    Klipper,
    PrintPaused,
    PrintError,
    AfcError,
    ChangerError,
    SpoolmanOffline,
}

impl Fault {
    pub fn kind(&self) -> FaultKind {
        match self {
            Self::MoonrakerUnreachable => FaultKind::MoonrakerUnreachable,
            Self::Klipper { .. } => FaultKind::Klipper,
            Self::Print(interruption) if interruption.failed => FaultKind::PrintError,
            Self::Print(_) => FaultKind::PrintPaused,
            Self::AfcError => FaultKind::AfcError,
            Self::ChangerError => FaultKind::ChangerError,
            Self::SpoolmanOffline => FaultKind::SpoolmanOffline,
        }
    }

    /// A pause and an unreachable Spoolman ask for a look; the rest failed.
    pub fn level(&self) -> FaultLevel {
        match self.kind() {
            FaultKind::PrintPaused | FaultKind::SpoolmanOffline => FaultLevel::Attention,
            _ => FaultLevel::Error,
        }
    }

    pub fn place(&self) -> Place {
        match self {
            Self::MoonrakerUnreachable | Self::Klipper { .. } => Place::Connection,
            Self::Print(_) => Place::Print,
            Self::AfcError => Place::AfcService,
            Self::ChangerError => Place::ChangerService,
            Self::SpoolmanOffline => Place::Spools,
        }
    }

    /// Whether the fault is the state the status bar names anyway (connection, Klipper, print); the others come on top
    /// of a state that may be fine.
    pub fn is_machine_state(&self) -> bool {
        matches!(self, Self::MoonrakerUnreachable | Self::Klipper { .. } | Self::Print(_))
    }
}

/// Every fault that stands in this snapshot, the most important first. Without Klipper ready nothing below it is
/// known for sure, so only the connection or Klipper counts then.
pub fn standing(snapshot: &Snapshot, spoolman_offline: bool) -> Vec<Fault> {
    if snapshot.connection == ConnectionState::Disconnected {
        return vec![Fault::MoonrakerUnreachable];
    }
    if snapshot.connection != ConnectionState::Connected {
        return Vec::new();
    }
    let status = &snapshot.status;
    match status.klippy_state {
        KlippyState::Shutdown | KlippyState::Error => {
            let message = status.klippy_message.lines().next().unwrap_or_default().to_owned();
            return vec![Fault::Klipper { shutdown: status.klippy_state == KlippyState::Shutdown, message }];
        }
        KlippyState::Ready => {}
        KlippyState::Disconnected | KlippyState::Startup => return Vec::new(),
    }
    let mut faults: Vec<Fault> = pause::interruption(status).map(Fault::Print).into_iter().collect();
    if changer::afc_service(status).is_some_and(|service| service.error) {
        faults.push(Fault::AfcError);
    }
    if status.toolchanger.as_ref().and_then(heads::changer_note) == Some(ChangerNote::Error) {
        faults.push(Fault::ChangerError);
    }
    if spoolman_offline {
        faults.push(Fault::SpoolmanOffline);
    }
    faults
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::klipper::PrintState;

    fn ready() -> Snapshot {
        let mut snapshot = Snapshot { connection: ConnectionState::Connected, ..Snapshot::default() };
        snapshot.status.klippy_state = KlippyState::Ready;
        snapshot
    }

    #[test]
    fn a_ready_printer_without_problems_has_no_fault() {
        assert!(standing(&ready(), false).is_empty());
    }

    #[test]
    fn an_unreachable_moonraker_is_the_only_fault() {
        let snapshot = Snapshot { connection: ConnectionState::Disconnected, ..ready() };

        assert_eq!(standing(&snapshot, true), [Fault::MoonrakerUnreachable]);
    }

    #[test]
    fn connecting_and_logging_in_are_no_faults() {
        for connection in [ConnectionState::Connecting, ConnectionState::LoginRequired] {
            let snapshot = Snapshot { connection, ..ready() };
            assert!(standing(&snapshot, false).is_empty(), "{connection:?}");
        }
    }

    #[test]
    fn klipper_in_shutdown_reports_the_first_line_of_its_message() {
        let mut snapshot = ready();
        snapshot.status.klippy_state = KlippyState::Shutdown;
        snapshot.status.klippy_message = "MCU 'mcu' shutdown: Timer too close\nOnce the underlying issue…".into();

        let faults = standing(&snapshot, true);

        assert_eq!(faults, [Fault::Klipper { shutdown: true, message: "MCU 'mcu' shutdown: Timer too close".into() }]);
        assert_eq!(faults[0].place(), Place::Connection);
    }

    #[test]
    fn a_starting_klipper_is_no_fault() {
        let mut snapshot = ready();
        snapshot.status.klippy_state = KlippyState::Startup;

        assert!(standing(&snapshot, true).is_empty());
    }

    #[test]
    fn a_paused_print_asks_for_attention_and_an_error_failed() {
        let mut snapshot = ready();
        snapshot.status.job.state = PrintState::Paused;
        let paused = standing(&snapshot, false);
        snapshot.status.job.state = PrintState::Error;
        let failed = standing(&snapshot, false);

        assert_eq!((paused[0].kind(), paused[0].level()), (FaultKind::PrintPaused, FaultLevel::Attention));
        assert_eq!((failed[0].kind(), failed[0].level()), (FaultKind::PrintError, FaultLevel::Error));
        assert_eq!(paused[0].place(), Place::Print);
    }

    #[test]
    fn spoolman_offline_comes_after_the_print() {
        let mut snapshot = ready();
        snapshot.status.job.state = PrintState::Paused;

        let kinds: Vec<FaultKind> = standing(&snapshot, true).iter().map(Fault::kind).collect();

        assert_eq!(kinds, [FaultKind::PrintPaused, FaultKind::SpoolmanOffline]);
    }
}
