//! The filament of a lane as the printer works with it: the copy its source keeps (Happy Hare's gate map, AFC's lane),
//! completed by the lane's own spool from Spoolman with what the copy lacks (grams left, the colours of a multi-colour
//! spool). Where the copy and the spool disagree, both are kept for a comparison; the spool never covers the copy
//! (Frank, 2026-11-23: a copy that did not take the spool's values must stay visible, Happy-Hare#1571).
//! Lanes without a copy (the direct feed, a head of a tool changer) show their spool.

use super::filament_match::{close_colors, same_material};
use super::machine_tree::{Lane, SpoolHolder};
use crate::color::Rgba;
use crate::sources::PrinterStatus;
use crate::sources::afc::AfcLane;
use crate::sources::happy_hare::{GateAvailability, MmuGate};
use crate::sources::spoolman::Spool;

/// Temperatures closer than this count as the same, in °C: Happy Hare and AFC keep whole degrees.
const SAME_TEMPERATURE: f64 = 0.5;

/// What a lane holds, as far as it is known.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LaneFilament {
    pub material: Option<String>,
    /// The colours of the filament with their alpha; several for a multi-colour spool of Spoolman.
    pub colors: Vec<Rgba>,
    /// The colours follow each other along the filament instead of lying side by side.
    pub longitudinal: bool,
    /// Vendor and name of the spool, or the filament name the source has for the lane.
    pub name: Option<String>,
    /// Grams left: from Spoolman, else what AFC counts down itself.
    pub left: Option<Remaining>,
    /// The source reports the lane as empty.
    pub empty: bool,
    /// Where the lane's spool in Spoolman says something else than the copy of the source.
    pub spool_differences: Vec<SpoolDifference>,
}

/// Grams of filament left on a spool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Remaining {
    pub grams: f64,
    /// Counted by the source from the extruder's movement (AFC), not measured or booked by Spoolman.
    pub estimated: bool,
}

/// A value the lane's spool has other than the copy of the source.
#[derive(Debug, Clone, PartialEq)]
pub enum SpoolDifference {
    /// `copy` is `None` when the source has no material for the lane.
    Material {
        copy: Option<String>,
        spool: String,
    },
    Color {
        copy: Option<Rgba>,
        spool: Option<Rgba>,
    },
    /// The nozzle temperature Happy Hare would take from the spool against the one of the gate, in °C.
    Temperature {
        copy: Option<f64>,
        spool: f64,
    },
}

impl LaneFilament {
    /// The grams left, measured or estimated alike; for comparisons with what a print needs.
    pub fn grams_left(&self) -> Option<f64> {
        self.left.map(|left| left.grams)
    }

    /// Only from Spoolman: a lane without a copy of its own.
    fn of_spool(spool: &Spool) -> Self {
        Self {
            material: Some(spool.material.clone()).filter(|material| !material.is_empty()),
            colors: spool.colors.clone(),
            longitudinal: spool.has_longitudinal_colors(),
            name: Some(spool.title()).filter(|title| !title.is_empty()),
            left: spool.remaining_weight.map(|grams| Remaining { grams, estimated: false }),
            empty: false,
            spool_differences: Vec::new(),
        }
    }
}

/// The filament of `lane`. Its copy comes from the source in `status`; `lane.spool` must hold the spool Spoolman
/// knows ([`super::machine_tree::MachineTree::lanes`]).
pub fn lane_filament(lane: &Lane, status: &PrinterStatus) -> LaneFilament {
    let own_spool = lane.spool.as_ref().filter(|_| lane.spool_holder() != Some(SpoolHolder::ActiveSpool));
    if let Some(gate) = lane.gate {
        let mmu = status.mmu.as_ref();
        if let Some(state) = mmu.and_then(|mmu| mmu.gates.iter().find(|state| state.index == gate)) {
            return gate_filament(state, mmu.map(|mmu| mmu.default_extruder_temp), own_spool);
        }
    }
    if let Some(state) = lane.name.as_ref().and_then(|name| status.afc.as_ref()?.lane(name)) {
        return afc_filament(state, own_spool);
    }
    let mut filament = lane.spool.as_ref().map(LaneFilament::of_spool).unwrap_or_default();
    filament.empty = lane.empty;
    filament
}

/// The nozzle temperature a spool brings to a lane: a gate of Happy Hare raises it to `default_extruder_temp`
/// (`mmu_server.py`: `max(temp, default_extruder_temp)`), elsewhere the spool's own.
pub fn spool_temperature(lane: &Lane, spool: &Spool, status: &PrinterStatus) -> Option<f64> {
    let temperature = spool.extruder_temp?;
    match (lane.spool_holder(), status.happy_hare()) {
        (Some(SpoolHolder::Gate(_)), Some(mmu)) => Some(temperature.max(mmu.default_extruder_temp)),
        _ => Some(temperature),
    }
}

fn gate_filament(gate: &MmuGate, default_temp: Option<f64>, spool: Option<&Spool>) -> LaneFilament {
    // Happy Hare takes only `color_hex`, which Spoolman leaves empty for a multi-colour filament (`mmu_server.py`).
    let spool_color = spool.and_then(|spool| if spool.is_multi_color() { None } else { spool.colors.first().copied() });
    let mut filament = from_copy(&gate.material, gate.color, &gate.name, spool, spool_color);
    filament.empty = gate.availability == GateAvailability::Empty;
    if let Some(spool) = spool {
        let raised = spool.extruder_temp.map(|temperature| temperature.max(default_temp.unwrap_or(temperature)));
        if let Some(expected) = raised.filter(|expected| !same_temperature(gate.temperature, *expected)) {
            filament.spool_differences.push(SpoolDifference::Temperature { copy: gate.temperature, spool: expected });
        }
    }
    filament
}

fn afc_filament(lane: &AfcLane, spool: Option<&Spool>) -> LaneFilament {
    // AFC takes the first colour of a multi-colour filament (`AFC_spool.py` `set_spool_id`).
    let spool_color = spool.and_then(|spool| spool.colors.first().copied());
    let mut filament = from_copy(&lane.material, lane.color, "", spool, spool_color);
    filament.empty = !lane.filled;
    if filament.left.is_none() {
        filament.left = lane.weight.map(|grams| Remaining { grams, estimated: true });
    }
    filament
}

/// The copy of a source with what the spool adds; `spool_color` is the colour the source would take from the spool.
fn from_copy(
    material: &str,
    color: Option<Rgba>,
    name: &str,
    spool: Option<&Spool>,
    spool_color: Option<Rgba>,
) -> LaneFilament {
    let copy_material = Some(material.to_owned()).filter(|material| !material.is_empty());
    let mut filament = LaneFilament {
        material: copy_material.clone(),
        colors: color.into_iter().collect(),
        longitudinal: false,
        name: Some(name.to_owned()).filter(|name| !name.is_empty()),
        ..LaneFilament::default()
    };
    let Some(spool) = spool else { return filament };
    if !spool.material.is_empty() && !copy_material.as_deref().is_some_and(|copy| same_material(copy, &spool.material))
    {
        filament
            .spool_differences
            .push(SpoolDifference::Material { copy: copy_material, spool: spool.material.clone() });
    }
    let colors_agree = match (color, spool_color) {
        (Some(copy), Some(expected)) => close_colors(copy, expected),
        (copy, expected) => copy.is_none() && expected.is_none(),
    };
    if colors_agree {
        if spool.is_multi_color() {
            filament.colors = spool.colors.clone();
            filament.longitudinal = spool.has_longitudinal_colors();
        }
    } else {
        filament.spool_differences.push(SpoolDifference::Color { copy: color, spool: spool.colors.first().copied() });
    }
    filament.name = Some(spool.title()).filter(|title| !title.is_empty()).or(filament.name);
    filament.left = spool.remaining_weight.map(|grams| Remaining { grams, estimated: false });
    filament
}

fn same_temperature(copy: Option<f64>, expected: f64) -> bool {
    copy.is_some_and(|copy| (copy - expected).abs() < SAME_TEMPERATURE)
}

/// What the user may do with the spool and the values of a lane that has a copy of its own (a gate, an AFC lane).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LaneSpoolRules {
    /// A spool of Spoolman can be assigned to the lane itself (not in Happy Hare's `off`).
    pub picks_spool: bool,
    /// Material, colour and temperature may be set by hand: without a spool, or Happy Hare's mode leaves them to it.
    pub values_editable: bool,
    /// The values of the lane's spool can be taken again now (not during a print with Happy Hare).
    pub refreshes: bool,
}

/// The rules for `lane` with `spoolman` configured in Moonraker or not; for a gate they follow Happy Hare's mode
/// ([`SpoolmanMode`](crate::sources::happy_hare::SpoolmanMode)), for an AFC lane its spool (AFC overwrites the
/// values with those of the spool on the next `SET_SPOOL_ID` and on restart).
pub fn lane_spool_rules(lane: &Lane, status: &PrinterStatus, spoolman: bool) -> LaneSpoolRules {
    if let (Some(gate), Some(mmu)) = (lane.gate, status.happy_hare()) {
        let state = mmu.gates.iter().find(|state| state.index == gate);
        let has_spool = mmu.spoolman.spools_at_gates() && state.is_some_and(|state| state.spool_id.is_some());
        return LaneSpoolRules {
            picks_spool: spoolman && mmu.spoolman.spools_at_gates(),
            values_editable: state.is_some_and(|state| mmu.gate_values_editable(state)),
            refreshes: spoolman && has_spool && !status.print_running(),
        };
    }
    let has_spool = lane.spool_id.is_some();
    LaneSpoolRules { picks_spool: spoolman, values_editable: !spoolman || !has_spool, refreshes: spoolman && has_spool }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::spools::lanes;
    use crate::sources::afc::AfcStatus;
    use crate::sources::happy_hare::{MmuStatus, SpoolmanMode};
    use crate::sources::spoolman::{SpoolmanState, test_spool};

    const RED: Rgba = [255, 0, 0, 255];
    const BLUE: Rgba = [0, 0, 255, 255];

    fn spoolman(spool: Spool) -> SpoolmanState {
        SpoolmanState { connected: true, spools: [(spool.id, spool)].into(), ..SpoolmanState::default() }
    }

    fn red_pla(id: u32) -> Spool {
        Spool { extruder_temp: Some(190.0), ..test_spool(id, "PLA", 640.0, [255, 0, 0]) }
    }

    fn happy_hare(mode: SpoolmanMode, gate: MmuGate) -> PrinterStatus {
        PrinterStatus {
            mmu: Some(MmuStatus {
                version: "4.2.0".into(),
                enabled: true,
                num_gates: 1,
                spoolman: mode,
                default_extruder_temp: 200.0,
                gates: vec![gate],
                ..MmuStatus::default()
            }),
            ..PrinterStatus::default()
        }
    }

    fn gate_with(material: &str, color: Option<Rgba>, temperature: Option<f64>, spool_id: Option<u32>) -> MmuGate {
        MmuGate { material: material.into(), color, temperature, spool_id, ..MmuGate::default() }
    }

    fn first_lane(status: &PrinterStatus, state: &SpoolmanState) -> LaneFilament {
        lane_filament(&lanes(status, state)[0], status)
    }

    #[test]
    fn a_gate_that_took_its_spool_shows_the_copy_with_the_grams_of_the_spool() {
        // Happy Hare raised the 190 °C of the spool to its default of 200 °C.
        let status = happy_hare(SpoolmanMode::Push, gate_with("PLA", Some(RED), Some(200.0), Some(7)));
        let filament = first_lane(&status, &spoolman(red_pla(7)));

        assert_eq!(filament.material.as_deref(), Some("PLA"));
        assert_eq!(filament.colors, [RED]);
        assert_eq!(filament.left, Some(Remaining { grams: 640.0, estimated: false }));
        assert!(filament.spool_differences.is_empty());
    }

    #[test]
    fn a_gate_that_missed_the_values_of_its_spool_shows_both() {
        // Happy-Hare#1571: in readonly the gate keeps only the spool id.
        let status = happy_hare(SpoolmanMode::Readonly, gate_with("", None, None, Some(7)));
        let filament = first_lane(&status, &spoolman(red_pla(7)));

        assert_eq!(filament.material, None);
        assert!(filament.colors.is_empty());
        assert_eq!(
            filament.spool_differences,
            [
                SpoolDifference::Material { copy: None, spool: "PLA".into() },
                SpoolDifference::Color { copy: None, spool: Some(RED) },
                SpoolDifference::Temperature { copy: None, spool: 200.0 },
            ]
        );
    }

    #[test]
    fn a_multi_colour_spool_shows_its_colours_where_the_copy_agrees() {
        let rainbow = Spool { colors: vec![RED, BLUE], ..red_pla(7) };
        let agreeing = happy_hare(SpoolmanMode::Push, gate_with("PLA", None, Some(200.0), Some(7)));
        assert_eq!(first_lane(&agreeing, &spoolman(rainbow.clone())).colors, [RED, BLUE]);

        let blue_gate = happy_hare(SpoolmanMode::Push, gate_with("PLA", Some(BLUE), Some(200.0), Some(7)));
        let filament = first_lane(&blue_gate, &spoolman(rainbow));
        assert_eq!(filament.colors, [BLUE]);
        assert_eq!(filament.spool_differences, [SpoolDifference::Color { copy: Some(BLUE), spool: Some(RED) }]);
    }

    #[test]
    fn in_off_the_gate_shows_only_its_copy() {
        let status = happy_hare(SpoolmanMode::Off, gate_with("PETG", Some(BLUE), Some(240.0), Some(7)));
        let state = SpoolmanState { active_spool_id: Some(7), ..spoolman(red_pla(7)) };
        let filament = first_lane(&status, &state);

        assert_eq!(filament.material.as_deref(), Some("PETG"));
        assert_eq!(filament.left, None);
        assert!(filament.spool_differences.is_empty());
    }

    #[test]
    fn an_afc_lane_counts_down_its_own_weight_without_a_spool() {
        let lane = AfcLane {
            name: "lane1".into(),
            extruder: "extruder".into(),
            material: "PLA".into(),
            filled: true,
            weight: Some(812.0),
            ..AfcLane::default()
        };
        let status =
            PrinterStatus { afc: Some(AfcStatus { lanes: vec![lane], ..AfcStatus::default() }), ..Default::default() };
        let filament = first_lane(&status, &SpoolmanState::default());

        assert_eq!(filament.left, Some(Remaining { grams: 812.0, estimated: true }));
        assert_eq!(filament.material.as_deref(), Some("PLA"));
    }

    #[test]
    fn an_afc_lane_with_a_spool_takes_the_grams_of_spoolman() {
        let lane = AfcLane {
            name: "lane1".into(),
            extruder: "extruder".into(),
            material: "PLA".into(),
            color: Some(RED),
            filled: true,
            spool_id: Some(7),
            weight: Some(812.0),
            ..AfcLane::default()
        };
        let status =
            PrinterStatus { afc: Some(AfcStatus { lanes: vec![lane], ..AfcStatus::default() }), ..Default::default() };
        let filament = first_lane(&status, &spoolman(red_pla(7)));

        assert_eq!(filament.left, Some(Remaining { grams: 640.0, estimated: false }));
        assert!(filament.spool_differences.is_empty());
    }

    #[test]
    fn the_rules_of_a_gate_follow_the_mode_of_happy_hare() {
        let rules = |mode, spool_id| {
            let status = happy_hare(mode, gate_with("PLA", None, None, spool_id));
            lane_spool_rules(&lanes(&status, &SpoolmanState::default())[0], &status, true)
        };
        let rule = |picks_spool, values_editable, refreshes| LaneSpoolRules { picks_spool, values_editable, refreshes };

        assert_eq!(rules(SpoolmanMode::Off, Some(7)), rule(false, true, false));
        assert_eq!(rules(SpoolmanMode::Readonly, None), rule(true, true, false));
        assert_eq!(rules(SpoolmanMode::Push, Some(7)), rule(true, false, true));
        assert_eq!(rules(SpoolmanMode::Pull, None), rule(true, false, false));
    }

    #[test]
    fn an_afc_lane_with_a_spool_keeps_its_values_from_spoolman() {
        let lane =
            |spool_id| AfcLane { name: "lane1".into(), extruder: "extruder".into(), spool_id, ..AfcLane::default() };
        let rules = |spool_id, spoolman| {
            let status = PrinterStatus {
                afc: Some(AfcStatus { lanes: vec![lane(spool_id)], ..AfcStatus::default() }),
                ..PrinterStatus::default()
            };
            lane_spool_rules(&lanes(&status, &SpoolmanState::default())[0], &status, spoolman)
        };

        assert_eq!(rules(Some(7), true), LaneSpoolRules { picks_spool: true, values_editable: false, refreshes: true });
        assert_eq!(rules(None, true), LaneSpoolRules { picks_spool: true, values_editable: true, refreshes: false });
        assert_eq!(rules(None, false), LaneSpoolRules { picks_spool: false, values_editable: true, refreshes: false });
    }

    #[test]
    fn a_printer_without_a_changer_shows_the_active_spool() {
        let state = SpoolmanState { active_spool_id: Some(7), ..spoolman(red_pla(7)) };
        let filament = first_lane(&PrinterStatus::default(), &state);

        assert_eq!(filament.material.as_deref(), Some("PLA"));
        assert_eq!(filament.left, Some(Remaining { grams: 640.0, estimated: false }));
    }
}
