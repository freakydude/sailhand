//! Spoolman: the spool in use, the picker and the hints before a print.

use super::mmu::filament_path_midway;
use crate::session::Session;
use sailhand::{Actions, Confirm, FilamentTab, MmuDialogs, Navigation, Page, Printer, SpoolPicker};
use sailhand_core::domain::machine_tree::LaneId;
use sailhand_core::sources::happy_hare::LoadedFilament;
use sailhand_core::{Command, Snapshot};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::time::Duration;

/// Without Spoolman in Moonraker there is nothing to show and the scene is skipped.
pub(super) fn spools(session: &mut Session) -> Result<(), String> {
    session.settle_replies(Duration::from_secs(2));
    let snapshot = session.presenter.snapshot();
    if !snapshot.sources.moonraker.spoolman {
        return Ok(());
    }
    let has_mmu = snapshot.status.mmu.is_some();
    if has_mmu {
        session.service.send(Command::MmuHome);
        session.wait_until("MMU homed", |snapshot| snapshot.status.mmu.as_ref().is_some_and(|mmu| mmu.homed))?;
        session.service.send(Command::MmuSelectGate { gate: 1 });
        session
            .wait_until("gate 1 selected", |snapshot| snapshot.status.mmu.as_ref().is_some_and(|mmu| mmu.gate == 1))?;
        filament_path_midway(session)?;
    } else {
        session.service.send(Command::SetActiveSpool { spool_id: Some(6) });
    }
    session.settle_replies(Duration::from_secs(2));
    session.service.send(Command::SetHeaterTarget { heater: "extruder".into(), target: 210.0 });
    session.pump(Duration::from_secs(1));
    session.show_page(Page::Print, "print-spool")?;
    session.show_page(Page::Temperature, "temperature-spool")?;

    session.window.global::<Navigation>().set_page(Page::Print);
    // Opens like a tap on the strip or gate 1: the spool in use is selected.
    let current = session.window.global::<Printer>().get_spool_id();
    session.window.global::<SpoolPicker>().invoke_show(if has_mmu { 1 } else { -1 }, current);
    session.settle_replies(Duration::from_secs(2));
    session.capture("spool-picker")?;
    session.window.global::<SpoolPicker>().set_open(false);

    session.presenter.request_files();
    session.settle_replies(Duration::from_secs(2));
    let tools = session.presenter.start_tools("long_job.gcode");
    session.window.global::<Confirm>().invoke_ask_to_start(
        "long_job".into(),
        "long_job.gcode".into(),
        ModelRc::new(VecModel::from(tools)),
    );
    session.pump(Duration::from_millis(300));
    session.capture("confirm-start-hints")?;
    session.window.global::<Confirm>().set_open(false);
    session.service.send(Command::SetHeaterTarget { heater: "extruder".into(), target: 0.0 });
    session.pump(Duration::from_millis(500));
    if has_mmu {
        spool_differences(session)?;
    }
    // Only where the spool in use is Moonraker's active spool: a lane of AFC or the head of a tool changer keeps its own.
    let status = session.presenter.snapshot().status;
    if !has_mmu && status.afc.is_none() && status.toolchanger.is_none() {
        unlink_active_spool(session)?;
    }
    Ok(())
}

/// Happy Hare with spools at its gates: the fake starts gate 1 with values its spool no longer has (edited in Spoolman
/// afterwards), so its dialog compares both. Then the loaded gate gets another spool, which Happy Hare does not book on
/// until the next load: the dialog offers to set it as the active spool. Gate 1 gets its spool back at the end.
fn spool_differences(session: &mut Session) -> Result<(), String> {
    let spools_at_gates = session.presenter.snapshot().status.mmu.is_some_and(|mmu| mmu.spoolman.spools_at_gates());
    if !spools_at_gates {
        return Ok(());
    }
    session.window.global::<Navigation>().set_page(Page::Filament);
    session.window.global::<Navigation>().set_filament_tab(FilamentTab::Gates);
    show_gate_dialog(session, 1, "filament-mmu-gate-spool-differs")?;

    session.service.send(Command::MmuLoad);
    session.wait_until("gate 1 loaded", |snapshot| {
        snapshot.status.mmu.as_ref().is_some_and(|mmu| mmu.filament == LoadedFilament::Loaded)
    })?;
    session.service.send(Command::AssignLaneSpool { lane: LaneId::Gate(1), spool_id: Some(4) });
    session.wait_until("spool 4 in gate 1", |snapshot| gate_spool(snapshot, 1) == Some(4))?;
    session.settle_replies(Duration::from_secs(2));
    show_gate_dialog(session, 1, "filament-mmu-gate-active-spool")?;

    session.service.send(Command::AssignLaneSpool { lane: LaneId::Gate(1), spool_id: Some(7) });
    session.service.send(Command::MmuUnload);
    session.wait_until("gate 1 unloaded with its spool", |snapshot| {
        gate_spool(snapshot, 1) == Some(7)
            && snapshot.status.mmu.as_ref().is_some_and(|mmu| mmu.filament == LoadedFilament::Unloaded)
    })?;
    session.settle_replies(Duration::from_secs(2));
    Ok(())
}

fn gate_spool(snapshot: &Snapshot, gate: usize) -> Option<u32> {
    snapshot.status.mmu.as_ref().and_then(|mmu| mmu.gates.get(gate)).and_then(|gate| gate.spool_id)
}

fn show_gate_dialog(session: &mut Session, row: i32, name: &str) -> Result<(), String> {
    session.window.global::<MmuDialogs>().set_gate_row(row);
    session.pump(Duration::from_millis(500));
    let captured = session.capture(name);
    session.window.global::<MmuDialogs>().set_gate_row(-1);
    captured
}

/// Without a filament changer or tool changer, "Unlink" in the picker clears the active spool of Moonraker. Without the reference layout the tap goes
/// through the app's callback.
fn unlink_active_spool(session: &mut Session) -> Result<(), String> {
    let current = session.window.global::<Printer>().get_spool_id();
    session.window.global::<SpoolPicker>().invoke_show(-1, current);
    session.pump(Duration::from_millis(300));
    if session.has_reference_layout() {
        session.tap(UNLINK_BUTTON.0, UNLINK_BUTTON.1);
        if session.window.global::<SpoolPicker>().get_open() {
            return Err("spool picker still open after Unlink".into());
        }
    } else {
        session.window.global::<Actions>().invoke_set_active_spool(-1);
        session.window.global::<SpoolPicker>().set_open(false);
    }
    session.settle_replies(Duration::from_secs(2));
    match session.window.global::<Printer>().get_spool_id() {
        -1 => Ok(()),
        id => Err(format!("spool {id} still active after Unlink")),
    }
}

/// The first button of the picker's bottom row in the 800x480 layout, right of the navigation rail.
const UNLINK_BUTTON: (f32, f32) = (200.0, 432.0);
