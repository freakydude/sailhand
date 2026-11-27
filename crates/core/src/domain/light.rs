//! The main light, the quick switch in the status bar, among Klipper's LEDs and Moonraker's WLED strips: found once by
//! name, then taken from the printer's settings ([`crate::settings::SavedPrinter::light`]).

use crate::sources::Sources;
use crate::sources::klipper::{Led, instance};

/// Names that mark the light of the build chamber.
const LIGHT_NAMES: [&str; 4] = ["chamber", "case", "light", "enclosure"];

/// Every light the printer has; Klipper's LEDs come before the WLED strips.
pub fn lights(sources: &Sources) -> impl Iterator<Item = &str> {
    sources.klipper.leds.iter().chain(&sources.wled_strips).map(String::as_str)
}

/// The light named like a chamber or case light, else the first.
pub fn detect(sources: &Sources) -> Option<&str> {
    let lights: Vec<&str> = lights(sources).collect();
    let named = lights.iter().find(|light| {
        let name = instance(light).to_ascii_lowercase();
        LIGHT_NAMES.iter().any(|light_name| name.contains(light_name))
    });
    named.or(lights.first()).copied()
}

/// The main light by the setting: `None` (no entry yet) detects it, an empty entry means none, a light the printer
/// lacks counts as none.
pub fn main_light<'a>(sources: &'a Sources, setting: Option<&str>) -> Option<&'a str> {
    match setting {
        None => detect(sources),
        Some(chosen) => lights(sources).find(|light| *light == chosen),
    }
}

/// The state of the main light among the reported lights; `None` if there is none or it reports nothing yet.
pub fn main_led<'a>(sources: &Sources, setting: Option<&str>, leds: &'a [Led]) -> Option<&'a Led> {
    let main_light = main_light(sources, setting)?;
    leds.iter().find(|led| led.object == main_light)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(leds: &[&str], wled_strips: &[&str]) -> Sources {
        let mut sources = Sources::default();
        sources.klipper.leds = leds.iter().map(|led| (*led).to_owned()).collect();
        sources.wled_strips = wled_strips.iter().map(|strip| (*strip).to_owned()).collect();
        sources
    }

    #[test]
    fn the_chamber_light_is_preferred_whatever_drives_it() {
        assert_eq!(detect(&sources(&["neopixel sb", "neopixel chamber"], &[])), Some("neopixel chamber"));
        assert_eq!(detect(&sources(&["neopixel sb"], &[])), Some("neopixel sb"));
        assert_eq!(detect(&sources(&["neopixel sb"], &["wled chamber"])), Some("wled chamber"));
        assert_eq!(detect(&sources(&["neopixel case"], &["wled chamber"])), Some("neopixel case"));
        assert_eq!(detect(&sources(&[], &["wled logo"])), Some("wled logo"));
        assert_eq!(detect(&sources(&[], &[])), None);
    }

    #[test]
    fn a_setting_decides_once_it_is_there() {
        let printer = sources(&["neopixel sb", "neopixel chamber"], &["wled logo"]);
        assert_eq!(main_light(&printer, None), Some("neopixel chamber"));
        assert_eq!(main_light(&printer, Some("wled logo")), Some("wled logo"));
        assert_eq!(main_light(&printer, Some("")), None);
        assert_eq!(main_light(&printer, Some("neopixel gone")), None);
    }
}
