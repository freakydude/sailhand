//! The chamber temperature of the status bar: found once by name among the heaters and sensors, then taken from the
//! printer's settings ([`crate::settings::SavedPrinter::chamber`]).

use super::temperature_limits::BarReading;
use crate::sources::klipper::instance;
use crate::sources::{PrinterStatus, Sources};

/// Names that mark the temperature of the build chamber.
const CHAMBER_NAMES: [&str; 2] = ["chamber", "enclosure"];

/// Every heater and sensor that can stand for the chamber, heaters first; the nozzles and the bed cannot.
pub fn candidates(sources: &Sources) -> impl Iterator<Item = &str> {
    let klipper = &sources.klipper;
    klipper.extra_heaters.iter().chain(&klipper.temperature_sensors).map(String::as_str)
}

/// The heater or sensor named like a chamber; a heater (it can be set) comes before a sensor.
pub fn detect(sources: &Sources) -> Option<&str> {
    candidates(sources).find(|object| {
        let name = instance(object).to_ascii_lowercase();
        CHAMBER_NAMES.iter().any(|chamber| name.contains(chamber))
    })
}

/// The chamber by the setting: `None` (no entry yet) detects it, an empty entry means none, an object the printer
/// lacks counts as none.
pub fn chamber<'a>(sources: &'a Sources, setting: Option<&str>) -> Option<&'a str> {
    match setting {
        None => detect(sources),
        Some(chosen) => candidates(sources).find(|object| *object == chosen),
    }
}

/// The reading of the chosen heater or sensor; none if it is gone or has no value.
pub fn reading(status: &PrinterStatus, object: &str) -> Option<BarReading> {
    if let Some(heater) = status.heater(object) {
        return Some(BarReading::of_heater(heater));
    }
    let sensor = status.sensors.iter().find(|sensor| sensor.object == object)?;
    sensor.temperature.map(BarReading::of_sensor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::klipper::{Heater, TemperatureSensor};

    fn sources(heaters: &[&str], sensors: &[&str]) -> Sources {
        let mut sources = Sources::default();
        sources.klipper.extra_heaters = heaters.iter().map(|name| (*name).to_owned()).collect();
        sources.klipper.temperature_sensors = sensors.iter().map(|name| (*name).to_owned()).collect();
        sources
    }

    #[test]
    fn a_chamber_heater_comes_before_a_chamber_sensor() {
        let found = sources(&["heater_generic Chamber_Heater"], &["temperature_sensor chamber"]);
        assert_eq!(detect(&found), Some("heater_generic Chamber_Heater"));
        assert_eq!(
            detect(&sources(&[], &["temperature_sensor cb1", "temperature_sensor enclosure"])),
            Some("temperature_sensor enclosure")
        );
        assert_eq!(detect(&sources(&["extruder1"], &["temperature_sensor cb1"])), None);
    }

    #[test]
    fn a_setting_decides_once_it_is_there() {
        let printer = sources(&[], &["temperature_sensor cb1", "temperature_sensor chamber"]);
        assert_eq!(chamber(&printer, None), Some("temperature_sensor chamber"));
        assert_eq!(chamber(&printer, Some("temperature_sensor cb1")), Some("temperature_sensor cb1"));
        assert_eq!(chamber(&printer, Some("")), None);
        assert_eq!(chamber(&printer, Some("temperature_sensor gone")), None);
    }

    #[test]
    fn the_reading_comes_from_a_heater_or_a_sensor() {
        let status = PrinterStatus {
            heaters: vec![Heater {
                object: "heater_generic chamber".into(),
                temperature: 40.0,
                target: 60.0,
                hysteresis: 5.0,
                ..Heater::default()
            }],
            sensors: vec![TemperatureSensor {
                object: "temperature_sensor enclosure".into(),
                temperature: Some(31.5),
                ..TemperatureSensor::default()
            }],
            ..PrinterStatus::default()
        };

        assert_eq!(reading(&status, "heater_generic chamber").unwrap().target, Some(60.0));
        assert_eq!(reading(&status, "temperature_sensor enclosure").unwrap().temperature, 31.5);
        assert_eq!(reading(&status, "temperature_sensor gone"), None);
    }
}
