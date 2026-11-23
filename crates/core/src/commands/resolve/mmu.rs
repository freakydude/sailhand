//! Happy Hare's gate map and EndlessSpool groups.

use super::lanes::is_new_material;
use crate::commands::{Command, CommandError};
use crate::sources::PrinterStatus;
use crate::sources::happy_hare::commands as mmu;
use crate::sources::happy_hare::{MmuGate, MmuStatus, SpoolmanMode};

pub(super) fn set_gate_filament(
    status: &PrinterStatus,
    gate: usize,
    material: Option<String>,
    color: Option<[u8; 3]>,
) -> Result<Vec<Command>, CommandError> {
    let state = status.mmu.as_ref().ok_or(CommandError::NoMmu)?;
    let current = editable_gate(state, gate)?;
    // The old temperature belongs to the old material (ASA at 255 °C stays on a gate switched to PLA otherwise).
    let temperature = if is_new_material(&current.material, material.as_deref()) {
        Some(state.default_extruder_temp)
    } else {
        current.temperature
    };
    Ok(vec![Command::RunGcode { script: mmu::set_gate_filament(gate, material.as_deref(), color, temperature)? }])
}

pub(super) fn set_gate_temperature(
    status: &PrinterStatus,
    gate: usize,
    temperature: f64,
) -> Result<Vec<Command>, CommandError> {
    let state = status.mmu.as_ref().ok_or(CommandError::NoMmu)?;
    editable_gate(state, gate)?;
    Ok(vec![Command::RunGcode { script: mmu::set_gate_filament(gate, None, None, Some(temperature))? }])
}

/// The gate, if its values may be set by hand in the mode of Happy Hare ([`SpoolmanMode::gate_values_editable`]).
fn editable_gate(state: &MmuStatus, gate: usize) -> Result<&MmuGate, CommandError> {
    let current = state.gates.get(gate).ok_or(CommandError::UnknownGate(gate))?;
    if !state.gate_values_editable(current) {
        return Err(CommandError::GateValuesFromSpoolman);
    }
    Ok(current)
}

/// Puts a spool into a gate the way the mode of Happy Hare takes it: `MMU_GATE_MAP` in `readonly` and `push`,
/// Spoolman's record in `pull`; `off` keeps no spools at the gates.
pub(super) fn assign_gate_spool(
    status: &PrinterStatus,
    gate: usize,
    spool_id: Option<u32>,
) -> Result<Vec<Command>, CommandError> {
    let state = status.mmu.as_ref().ok_or(CommandError::NoMmu)?;
    state.gates.get(gate).ok_or(CommandError::UnknownGate(gate))?;
    let script = match state.spoolman {
        SpoolmanMode::Off => return Err(CommandError::NoGateSpools),
        SpoolmanMode::Readonly | SpoolmanMode::Push => mmu::assign_gate_spool(gate, spool_id),
        SpoolmanMode::Pull => mmu::assign_gate_spool_in_spoolman(gate, spool_id),
    };
    Ok(vec![Command::RunGcode { script }])
}

/// Gives the gates the values of their spools again; only outside a print (Happy Hare's documentation of
/// `MMU_SPOOLMAN REFRESH`).
pub(super) fn refresh_gate_spool(status: &PrinterStatus, gate: usize) -> Result<Vec<Command>, CommandError> {
    let state = status.mmu.as_ref().ok_or(CommandError::NoMmu)?;
    let current = state.gates.get(gate).ok_or(CommandError::UnknownGate(gate))?;
    if !state.spoolman.spools_at_gates() {
        return Err(CommandError::NoGateSpools);
    }
    current.spool_id.ok_or(CommandError::NoLaneSpool)?;
    if status.print_running() {
        return Err(CommandError::PrintRunning);
    }
    Ok(vec![Command::RunGcode { script: mmu::REFRESH_FROM_SPOOLMAN.to_owned() }])
}

pub(super) fn set_endless_spool_group(
    status: &PrinterStatus,
    gate: usize,
    group: u32,
) -> Result<Vec<Command>, CommandError> {
    let state = status.mmu.as_ref().ok_or(CommandError::NoMmu)?;
    if gate >= state.gates.len() {
        return Err(CommandError::UnknownGate(gate));
    }
    let groups = state.endless_spool_groups_with(gate, group).ok_or(CommandError::NoEndlessSpoolGroups)?;
    Ok(vec![Command::RunGcode { script: mmu::endless_spool_groups(&groups) }])
}
