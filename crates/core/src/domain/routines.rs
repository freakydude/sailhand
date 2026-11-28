//! Routines: what a person starts on this display and Klipper then carries out on its own, the macros and the
//! calibrations. A message that came from one leads back to where it was started (Frank 2026-11-28).

use crate::commands::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routine {
    /// A macro, by its name.
    Macro(String),
    /// Bed screws, levelling, Z height and the bed mesh, with their interactive modes.
    Bed,
    /// `PID_CALIBRATE` of a heater, by its object name.
    Pid(String),
}

/// The routine a command of this display belongs to; `None` for everything else (moves, heaters, the console).
pub fn routine_of(command: &Command) -> Option<Routine> {
    match command {
        Command::RunMacro { name } => Some(Routine::Macro(name.clone())),
        Command::PidCalibrate { heater, .. } => Some(Routine::Pid(heater.clone())),
        Command::Level
        | Command::QuadGantryLevel
        | Command::ZTiltAdjust
        | Command::CalibrateBedMesh
        | Command::LoadBedMesh { .. }
        | Command::ClearBedMesh
        | Command::SaveBedMesh { .. }
        | Command::RemoveBedMesh { .. }
        | Command::CalculateScrewsTilt
        | Command::AdjustBedScrews
        | Command::CalibrateZ
        | Command::ProbeCalibrate
        | Command::ZEndstopCalibrate
        | Command::CalibrateEddyDriveCurrent
        | Command::EddyCalibrate { .. }
        | Command::EddyDriveCurrent { .. }
        | Command::MoveToBedCentre { .. }
        | Command::EddyTapCalibrate { .. }
        | Command::TestZ { .. }
        | Command::AcceptCalibration
        | Command::ScrewAdjusted
        | Command::AbortCalibration => Some(Routine::Bed),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macros_calibrations_and_other_commands() {
        assert_eq!(
            routine_of(&Command::RunMacro { name: "HEAT_SOAK".into() }),
            Some(Routine::Macro("HEAT_SOAK".into()))
        );
        assert_eq!(
            routine_of(&Command::PidCalibrate { heater: "extruder".into(), target: 230.0 }),
            Some(Routine::Pid("extruder".into()))
        );
        assert_eq!(routine_of(&Command::CalibrateBedMesh), Some(Routine::Bed));
        assert_eq!(routine_of(&Command::AbortCalibration), Some(Routine::Bed));
        assert_eq!(routine_of(&Command::SaveConfig), None, "Save config belongs to whatever is pending");
        assert_eq!(routine_of(&Command::CoolDown), None);
    }
}
