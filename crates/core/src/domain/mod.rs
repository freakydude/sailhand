//! Questions that need more than one source, such as how to load filament or how long a print will take. Each
//! question is answered in exactly one place, which also decides which source wins.

pub mod active_spool;
pub mod chamber;
pub mod changer;
pub mod covered_prompts;
pub mod estimates;
pub mod faults;
pub mod filament;
pub mod filament_match;
pub mod file_catalog;
pub mod gate_pages;
pub mod heads;
pub mod incidents;
pub mod job_end;
pub mod lane_filament;
pub mod light;
pub mod load_path;
pub mod machine_tree;
pub mod manual_probe;
pub mod pause;
pub mod pid_tuning;
pub mod prepared_job;
pub mod print_hints;
pub mod screen_wake;
pub mod spools;
pub mod temperature_limits;
