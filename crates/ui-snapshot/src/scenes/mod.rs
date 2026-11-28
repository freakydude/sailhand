//! The run, split into scenes that can be chosen on the command line.

use crate::session::Session;
use sailhand::{Actions, Confirm, SpoolPicker};
use sailhand_core::sources::klipper::KlippyState;
use sailhand_core::{Command, ConnectionState, Query};
use slint::ComponentHandle;
use std::time::Duration;

mod bed_mesh_dense;
mod calibrate;
mod console;
mod lanes;
mod login;
mod macros;
mod mmu;
mod network;
mod pages;
mod printing;
mod prompt;
mod spools;
mod system;
mod temperature;
mod toolchanger;

use bed_mesh_dense::bed_mesh_dense;
use calibrate::calibrate;
use console::typing;
use lanes::lanes;
use login::{log_in, logged_in};
use macros::macros;
use network::network;
use pages::pages;
use printing::printing;
use spools::spools;
use system::{idle, shutdown};
use temperature::taps;
use toolchanger::toolchanger;

/// Scenes run in this order; each one leaves the printer ready for the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scene {
    /// Every page in standby, with the file list.
    Pages,
    /// Spoolman: the spool in use, the temperature hint, the picker and the hints before a print.
    Spools,
    /// A tool changer (`-p toolchanger`): spool in use, hints, the picker of a tool macro, the prepared file.
    Toolchanger,
    /// Lanes and the load path (`-p afc`, `-p afc-toolchanger`, `-p toolchanger-mmu`, `-p toolchanger`): the tiles and
    /// dialog of AFC lanes, the path card, the head's part of the path and the sensors of the machine on System; with AFC
    /// also the Changer page (tool map, service) and the error state.
    Lanes,
    /// System > Network: list, connecting, confirmation, read-only fallback; System > Printers.
    Network,
    /// Taps that last longer than a status update still count while the temperatures change.
    Taps,
    /// Editing keys in the text entry of the console, physical and on-screen.
    Typing,
    /// Routines › Macros: the run card of a macro while it runs and after its answer.
    Macros,
    /// The Bed tab of the Routines page: results of the bed mesh and the screws, the dialogs of the waiting modes.
    Calibrate,
    /// A dense bed mesh (30 x 40 points, 31 x 31 on a fake with `--round-bed`), which the UI draws as an image.
    BedMeshDense,
    /// Print running, system page locked, cancel.
    Printing,
    /// Emergency stop and firmware restart.
    Shutdown,
    /// Idle timeout; slow, therefore not part of the default selection.
    Idle,
    /// Moonraker wants a login: refused, accepted, logged in on the settings, logged out. Needs a fake started with
    /// `--auth test:secret`, so it is only run when chosen by name.
    Login,
}

impl Scene {
    pub const ALL: [Self; 14] = [
        Self::Pages,
        Self::Spools,
        Self::Toolchanger,
        Self::Lanes,
        Self::Network,
        Self::Taps,
        Self::Typing,
        Self::Macros,
        Self::Calibrate,
        Self::BedMeshDense,
        Self::Printing,
        Self::Shutdown,
        Self::Idle,
        Self::Login,
    ];
    pub const DEFAULT: [Self; 11] = [
        Self::Pages,
        Self::Spools,
        Self::Toolchanger,
        Self::Lanes,
        Self::Network,
        Self::Taps,
        Self::Typing,
        Self::Macros,
        Self::Calibrate,
        Self::Printing,
        Self::Shutdown,
    ];

    /// Parses `pages,network` or `all` (every scene that runs against a fake without login).
    pub fn parse_list(text: &str) -> Result<Vec<Self>, String> {
        if text == "all" {
            return Ok(Self::ALL.into_iter().filter(|scene| *scene != Self::Login).collect());
        }
        text.split(',').map(|name| Self::parse(name.trim())).collect()
    }

    fn parse(name: &str) -> Result<Self, String> {
        Self::ALL.into_iter().find(|scene| scene.name() == name).ok_or_else(|| {
            format!(
                "unknown scene '{name}' (pages, spools, toolchanger, lanes, network, taps, typing, macros, calibrate, bed-mesh-dense, printing, shutdown, idle, \
                 login, all)"
            )
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Pages => "pages",
            Self::Spools => "spools",
            Self::Toolchanger => "toolchanger",
            Self::Lanes => "lanes",
            Self::Network => "network",
            Self::Taps => "taps",
            Self::Typing => "typing",
            Self::Macros => "macros",
            Self::Calibrate => "calibrate",
            Self::BedMeshDense => "bed-mesh-dense",
            Self::Printing => "printing",
            Self::Shutdown => "shutdown",
            Self::Idle => "idle",
            Self::Login => "login",
        }
    }

    fn run(self, session: &mut Session) -> Result<(), String> {
        match self {
            Self::Pages => pages(session),
            Self::Spools => spools(session),
            Self::Toolchanger => toolchanger(session),
            Self::Lanes => lanes(session),
            Self::Network => network(session),
            Self::Taps => taps(session),
            Self::Typing => typing(session),
            Self::Macros => macros(session),
            Self::Calibrate => calibrate(session),
            Self::BedMeshDense => bed_mesh_dense(session),
            Self::Printing => printing(session),
            Self::Shutdown => shutdown(session),
            Self::Idle => idle(session),
            Self::Login => logged_in(session),
        }
    }
}

/// Connects (logging in first for the scene `login`), homes and loads host data, then runs the chosen scenes in their
/// fixed order. With `keep_going` a failing scene is reported and the next one runs; the run still fails at the end.
pub fn run(session: &mut Session, chosen: &[Scene], keep_going: bool) -> Result<(), String> {
    session.capture("connecting")?;
    if chosen.contains(&Scene::Login) {
        log_in(session)?;
    }
    session.wait_until("printer ready", |snapshot| {
        snapshot.connection == ConnectionState::Connected && snapshot.status.klippy_state == KlippyState::Ready
    })?;
    session.service.send(Command::Home { x: true, y: true, z: true });
    session.service.query(Query::PowerDevices);
    session.service.query(Query::Updates);
    session.service.query(Query::ConsoleHistory);
    session.service.send(Command::RunGcode { script: "STATUS".into() });
    session.service.send(Command::RunGcode { script: "HELP".into() });
    session.wait_until("homing", |snapshot| snapshot.status.motion.homed.all())?;

    let mut failed = Vec::new();
    for scene in Scene::ALL.into_iter().filter(|scene| chosen.contains(scene)) {
        match scene.run(session) {
            Err(error) if keep_going => {
                eprintln!("ui-snapshot: scene {scene:?} failed: {error}");
                failed.push(format!("{scene:?}"));
                close_dialogs(session);
            }
            result => result?,
        }
    }
    if failed.is_empty() { Ok(()) } else { Err(format!("failed scenes: {}", failed.join(", "))) }
}

/// What a failed scene left open would cover the next scenes.
fn close_dialogs(session: &mut Session) {
    session.window.global::<Actions>().invoke_close_prompt();
    session.window.global::<Confirm>().set_open(false);
    session.window.global::<SpoolPicker>().set_open(false);
    session.pump(Duration::from_millis(600));
}

/// The light symbol of the status bar (800x480, no fitting).
const STATUS_BAR_LIGHT: (f32, f32) = (700.0, 22.0);

/// Opens the confirmation dialog as a page would.
fn ask(session: &Session, title: &str, message: &str, action: &str, argument: &str) {
    session.window.global::<Confirm>().invoke_ask(title.into(), message.into(), action.into(), argument.into(), true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scene_lists() {
        assert_eq!(Scene::parse_list("network, pages"), Ok(vec![Scene::Network, Scene::Pages]));
        assert_eq!(Scene::parse_list("all").map(|scenes| scenes.len()), Ok(Scene::ALL.len() - 1));
        assert_eq!(Scene::parse_list("login"), Ok(vec![Scene::Login]));
        assert!(Scene::parse_list("pages,unknown").is_err());
    }
}
