//! Whether Moonraker books the filament on the spool in use. Moonraker counts the extrusion of the active extruder on
//! one active spool; Happy Hare, AFC or a macro of the user set it, and a spool assigned to the loaded gate afterwards
//! stays unbooked until the next load (Happy Hare calls `spoolman_set_active_spool` only after loading and at the start
//! of unloading, `mmu_filament_movement.py`). So the spool of the lane in use is compared with the active spool
//! (Frank, 2026-11-23).

use super::machine_tree::{Lane, SpoolHolder};
use crate::sources::PrinterStatus;
use crate::sources::happy_hare::LoadedFilament;
use crate::sources::spoolman::SpoolmanState;

/// The lane in use holds another spool than the one Moonraker books on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveSpoolMismatch {
    /// The spool of the lane in use.
    pub lane_spool: u32,
    /// The spool Moonraker books on; `None` for none.
    pub active_spool: Option<u32>,
}

/// The mismatch, if there is one to tell: only while Spoolman is connected, the lane in use is loaded and keeps a spool
/// of its own, and its system rests (Happy Hare clears the active spool on purpose when unloading starts, a change of
/// head or lane is under way). A lane that holds the active spool itself (the direct feed, a gate in Happy Hare's
/// `off`) has nothing to compare.
pub fn active_spool_mismatch(
    status: &PrinterStatus,
    lanes: &[Lane],
    spoolman: &SpoolmanState,
) -> Option<ActiveSpoolMismatch> {
    if !spoolman.connected {
        return None;
    }
    let lane = lanes.iter().find(|lane| lane.active)?;
    if lane.spool_holder() == Some(SpoolHolder::ActiveSpool) || !loaded_and_at_rest(lane, status) {
        return None;
    }
    let lane_spool = lane.spool_id?;
    (spoolman.active_spool_id != Some(lane_spool))
        .then_some(ActiveSpoolMismatch { lane_spool, active_spool: spoolman.active_spool_id })
}

fn loaded_and_at_rest(lane: &Lane, status: &PrinterStatus) -> bool {
    if lane.gate.is_some() {
        return status.happy_hare().is_some_and(|mmu| mmu.filament == LoadedFilament::Loaded && mmu.is_at_rest());
    }
    if lane.name.is_some() {
        // The lane in use is AFC's `current_load`, so it is loaded.
        return status.afc.as_ref().is_some_and(|afc| afc.is_at_rest());
    }
    // A head of a tool changer is in use while mounted; whether filament is in it nobody reports.
    lane.head.is_none() || status.toolchanger.as_ref().is_none_or(|toolchanger| toolchanger.is_ready())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::spools::lanes;
    use crate::sources::afc::{AfcLane, AfcStatus};
    use crate::sources::happy_hare::{MmuGate, MmuStatus, SpoolmanMode};
    use crate::sources::tool_macros::ToolSpool;
    use crate::sources::toolchanger::{ToolHead, ToolchangerStatus};

    fn spoolman(active: Option<u32>) -> SpoolmanState {
        SpoolmanState { connected: true, active_spool_id: active, ..SpoolmanState::default() }
    }

    fn check(status: &PrinterStatus, state: &SpoolmanState) -> Option<ActiveSpoolMismatch> {
        active_spool_mismatch(status, &lanes(status, state), state)
    }

    fn happy_hare(mode: SpoolmanMode, action: &str, filament: LoadedFilament) -> PrinterStatus {
        PrinterStatus {
            mmu: Some(MmuStatus {
                version: "4.2.0".into(),
                enabled: true,
                num_gates: 2,
                gate: 1,
                action: action.into(),
                filament,
                spoolman: mode,
                gates: vec![
                    MmuGate { index: 0, spool_id: Some(3), ..MmuGate::default() },
                    MmuGate { index: 1, spool_id: Some(4), ..MmuGate::default() },
                ],
                ..MmuStatus::default()
            }),
            ..PrinterStatus::default()
        }
    }

    #[test]
    fn a_loaded_gate_at_rest_is_compared_with_the_active_spool() {
        let status = happy_hare(SpoolmanMode::Push, "Idle", LoadedFilament::Loaded);

        assert_eq!(check(&status, &spoolman(Some(4))), None);
        assert_eq!(
            check(&status, &spoolman(Some(3))),
            Some(ActiveSpoolMismatch { lane_spool: 4, active_spool: Some(3) })
        );
        assert_eq!(check(&status, &spoolman(None)), Some(ActiveSpoolMismatch { lane_spool: 4, active_spool: None }));
    }

    #[test]
    fn happy_hare_while_moving_unloaded_or_in_off_is_not_compared() {
        let none = spoolman(None);
        assert_eq!(check(&happy_hare(SpoolmanMode::Push, "Unloading", LoadedFilament::Loaded), &none), None);
        assert_eq!(check(&happy_hare(SpoolmanMode::Push, "Idle", LoadedFilament::Unloaded), &none), None);
        assert_eq!(check(&happy_hare(SpoolmanMode::Off, "Idle", LoadedFilament::Loaded), &none), None);
    }

    #[test]
    fn without_spoolman_connected_nothing_is_compared() {
        let status = happy_hare(SpoolmanMode::Push, "Idle", LoadedFilament::Loaded);
        assert_eq!(check(&status, &SpoolmanState::default()), None);
    }

    #[test]
    fn the_loaded_afc_lane_is_compared_while_afc_rests() {
        let lane = |name: &str, spool_id| AfcLane {
            name: name.into(),
            extruder: "extruder".into(),
            spool_id: Some(spool_id),
            filled: true,
            ..AfcLane::default()
        };
        let afc = |state: &str| AfcStatus {
            state: state.into(),
            current_load: Some("lane2".into()),
            lanes: vec![lane("lane1", 5), lane("lane2", 6)],
            ..AfcStatus::default()
        };
        let resting = PrinterStatus { afc: Some(afc("Idle")), ..PrinterStatus::default() };
        let swapping = PrinterStatus { afc: Some(afc("ToolSwap")), ..PrinterStatus::default() };

        assert_eq!(
            check(&resting, &spoolman(Some(5))),
            Some(ActiveSpoolMismatch { lane_spool: 6, active_spool: Some(5) })
        );
        assert_eq!(check(&swapping, &spoolman(Some(5))), None);
    }

    #[test]
    fn the_mounted_head_of_a_ready_tool_changer_is_compared() {
        let head = |number: usize, active| ToolHead {
            name: format!("T{number}"),
            number: Some(number),
            extruder: Some(if number == 0 { "extruder".into() } else { format!("extruder{number}") }),
            fan: None,
            active,
        };
        let status = |changer: &str| PrinterStatus {
            toolchanger: Some(ToolchangerStatus {
                status: changer.into(),
                heads: vec![head(0, false), head(1, true)],
                ..ToolchangerStatus::default()
            }),
            tool_spools: vec![
                ToolSpool { tool: 0, macro_name: "T0".into(), spool_id: Some(8), active: false },
                ToolSpool { tool: 1, macro_name: "T1".into(), spool_id: Some(9), active: true },
            ],
            ..PrinterStatus::default()
        };

        assert_eq!(
            check(&status("ready"), &spoolman(Some(8))),
            Some(ActiveSpoolMismatch { lane_spool: 9, active_spool: Some(8) })
        );
        assert_eq!(check(&status("changing"), &spoolman(Some(8))), None);
    }

    #[test]
    fn a_printer_without_changer_books_on_the_spool_it_shows() {
        assert_eq!(check(&PrinterStatus::default(), &spoolman(Some(8))), None);
    }
}
