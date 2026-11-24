//! How Happy Hare works with Spoolman: `spoolman_support` in `[mmu_parameters]` (table in `config/base/mmu.cfg`,
//! `mmu_controller.py`, `commands/mmu_gate_map.py`, `components/mmu_server.py`; Happy Hare 4.2, main 5aa3d30).

/// The four modes of `spoolman_support`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpoolmanMode {
    /// Happy Hare ignores Spoolman: no spools at the gates, no active spool.
    #[default]
    Off,
    /// Happy Hare takes the values of a gate's spool from Spoolman and sets the active spool on load.
    Readonly,
    /// Like `readonly`, and it also writes the gate of each spool to Spoolman.
    Push,
    /// Spoolman keeps the gate of each spool; Happy Hare takes the gate map from there and refuses changes to it.
    Pull,
}

impl SpoolmanMode {
    /// Unknown or missing values count as `off`, the default of Happy Hare.
    pub fn parse(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "readonly" => Self::Readonly,
            "push" => Self::Push,
            "pull" => Self::Pull,
            _ => Self::Off,
        }
    }

    /// A gate holds a spool of its own; in `off` the active spool is picked at the extruder instead.
    pub fn spools_at_gates(self) -> bool {
        self != Self::Off
    }

    /// Material, colour and temperature of a gate may be set by hand: always in `off`; in `readonly` and `push` only
    /// without a spool, since the next sync with Spoolman overwrites them; never in `pull`
    /// (`mmu_gate_map.py` refuses `MATERIAL`, `COLOR`, `NAME`, … then).
    pub fn gate_values_editable(self, gate_has_spool: bool) -> bool {
        match self {
            Self::Off => true,
            Self::Readonly | Self::Push => !gate_has_spool,
            Self::Pull => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_modes_are_read_and_anything_else_is_off() {
        assert_eq!(SpoolmanMode::parse("off"), SpoolmanMode::Off);
        assert_eq!(SpoolmanMode::parse("readonly"), SpoolmanMode::Readonly);
        assert_eq!(SpoolmanMode::parse("push"), SpoolmanMode::Push);
        assert_eq!(SpoolmanMode::parse(" Pull "), SpoolmanMode::Pull);
        assert_eq!(SpoolmanMode::parse(""), SpoolmanMode::Off);
        assert_eq!(SpoolmanMode::parse("sometimes"), SpoolmanMode::Off);
    }

    #[test]
    fn gate_values_are_editable_by_mode_and_spool() {
        use SpoolmanMode::*;
        let editable = |mode: SpoolmanMode| [mode.gate_values_editable(false), mode.gate_values_editable(true)];
        assert_eq!(editable(Off), [true, true]);
        assert_eq!(editable(Readonly), [true, false]);
        assert_eq!(editable(Push), [true, false]);
        assert_eq!(editable(Pull), [false, false]);
    }

    #[test]
    fn only_off_has_no_spools_at_the_gates() {
        for mode in [SpoolmanMode::Readonly, SpoolmanMode::Push, SpoolmanMode::Pull] {
            assert!(mode.spools_at_gates());
        }
        assert!(!SpoolmanMode::Off.spools_at_gates());
    }
}
