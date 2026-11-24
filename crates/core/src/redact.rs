//! Redaction for everything that leaves the machine in a bug report: the diagnostic report, the log header and the
//! exported log. Reports end up in public GitHub issues, so anything that identifies the printer, the network or the
//! user is replaced; when in doubt, it is replaced.
//!
//! Redacted: addresses (IP addresses, URLs, local host names), uuids, MAC addresses, serial numbers (also in
//! `/dev/serial/...` paths), credentials, and the user name in home paths. Free text is searched by pattern, JSON
//! also by key name. A bare host name without a recognisable suffix cannot be found in free text; its place in
//! structured data is covered by the key names.

use serde_json::Value;
use std::net::IpAddr;

/// What replaces a secret value.
pub const REDACTED: &str = "<redacted>";
const URL_MARK: &str = "<url>";
const ADDRESS_MARK: &str = "<address>";
const UUID_MARK: &str = "<uuid>";
const MAC_MARK: &str = "<mac>";
const HOME_MARK: &str = "~";
/// Characters that end a word in free text.
const WORD_BREAKS: &str = "\"'<>()[]{},;=|";
/// Host name endings of local networks.
const LOCAL_SUFFIXES: &[&str] = &[".local", ".lan", ".home", ".home.arpa", ".fritz.box", ".internal", ".localdomain"];
const HOME_FOLDERS: &[&str] = &["/home/", "/Users/"];
const SERIAL_FOLDERS: &[&str] = &["/dev/serial/by-id/", "/dev/serial/by-path/"];
const UUID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
const MAC_BYTES: usize = 6;
/// Flags that take the Moonraker address.
const MOONRAKER_FLAGS: &[&str] = &["-m", "--moonraker"];
/// Keys whose whole value is replaced (compared in lower case).
const SECRET_KEYS: &[&str] = &[
    "address",
    "host",
    "hostname",
    "host_name",
    "ip",
    "ip_address",
    "ip_addresses",
    "mac",
    "mac_address",
    "url",
    "server",
    "ssid",
    "user",
    "username",
    "login",
    "instance_id",
    "machine_id",
];
/// Parts of key names that mark a secret (compared in lower case).
const SECRET_KEY_PARTS: &[&str] = &["serial", "uuid", "password", "passwd", "secret", "token", "api_key", "apikey"];

/// Free text with everything privacy-relevant replaced; spacing and line breaks stay.
pub fn text(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut word = String::new();
    for character in input.chars() {
        if character.is_whitespace() || WORD_BREAKS.contains(character) {
            output.push_str(&word_redacted(&word));
            word.clear();
            output.push(character);
        } else {
            word.push(character);
        }
    }
    output.push_str(&word_redacted(&word));
    output
}

/// A JSON value with every string searched by pattern and every value of a secret key replaced, recursively.
pub fn value(input: &mut Value) {
    match input {
        Value::String(string) => *string = text(string),
        Value::Array(items) => items.iter_mut().for_each(value),
        Value::Object(object) => {
            for (key, entry) in object.iter_mut() {
                if is_secret_key(key) {
                    *entry = Value::String(REDACTED.to_owned());
                } else {
                    value(entry);
                }
            }
        }
        _ => {}
    }
}

/// A path for display with the user name of the home folder gone: `/home/frank/x` becomes `~/x`.
pub fn path(input: &str) -> String {
    let mut output = input.to_owned();
    for folder in HOME_FOLDERS {
        let mut from = 0;
        while let Some(offset) = output[from..].find(folder) {
            let start = from + offset;
            let name_start = start + folder.len();
            let name_end = output[name_start..].find('/').map_or(output.len(), |end| name_start + end);
            output.replace_range(start..name_end, HOME_MARK);
            from = start + HOME_MARK.len();
        }
    }
    output
}

/// The command line for the log header: the value of `--moonraker` is replaced, the rest searched by pattern.
pub fn command_line(arguments: &[String]) -> String {
    let mut shown = Vec::with_capacity(arguments.len());
    let mut address_follows = false;
    for argument in arguments {
        shown.push(if address_follows {
            REDACTED.to_owned()
        } else if let Some(flag) = flag_with_attached_address(argument) {
            format!("{flag}{REDACTED}")
        } else {
            text(argument)
        });
        address_follows = MOONRAKER_FLAGS.contains(&argument.as_str());
    }
    shown.join(" ")
}

/// `--moonraker=value` or `-mvalue`: the flag part of it, including `=`.
fn flag_with_attached_address(argument: &str) -> Option<&'static str> {
    if argument.starts_with("--moonraker=") {
        return Some("--moonraker=");
    }
    (argument.starts_with("-m") && argument.len() > "-m".len()).then_some("-m")
}

fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    SECRET_KEYS.contains(&key.as_str()) || SECRET_KEY_PARTS.iter().any(|part| key.contains(part))
}

fn word_redacted(word: &str) -> String {
    if word.is_empty() {
        return String::new();
    }
    if let Some(position) = word.find("://") {
        return url_redacted(word, position);
    }
    if let Some(mark) = whole_word_mark(word) {
        return format!("{mark}{}", &word[word.trim_end_matches(['.', ':']).len()..]);
    }
    path(&serial_path_redacted(word))
}

/// The scheme and everything after it become one mark; letters before `://` are the scheme.
fn url_redacted(word: &str, separator: usize) -> String {
    let scheme_start = word[..separator]
        .char_indices()
        .rev()
        .take_while(|(_, character)| character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.'))
        .last()
        .map_or(separator, |(index, _)| index);
    format!("{}{URL_MARK}", &word[..scheme_start])
}

/// The mark for a word that is an address, uuid or MAC address as a whole (ignoring punctuation at its end).
fn whole_word_mark(word: &str) -> Option<&'static str> {
    let core = word.trim_end_matches(['.', ':']);
    if is_address(core) {
        Some(ADDRESS_MARK)
    } else if is_uuid(core) {
        Some(UUID_MARK)
    } else if is_mac(core) {
        Some(MAC_MARK)
    } else {
        None
    }
}

/// An IP address or local host name, with an optional port.
fn is_address(word: &str) -> bool {
    if word.parse::<IpAddr>().is_ok() {
        return true;
    }
    let host = match word.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|digit| digit.is_ascii_digit()) => host,
        _ => word,
    };
    host.parse::<IpAddr>().is_ok() || is_local_host_name(host)
}

fn is_local_host_name(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    LOCAL_SUFFIXES.iter().any(|suffix| host.len() > suffix.len() && host.ends_with(suffix))
}

fn is_uuid(word: &str) -> bool {
    let groups: Vec<&str> = word.split('-').collect();
    groups.len() == UUID_GROUPS.len()
        && groups.iter().zip(UUID_GROUPS).all(|(group, length)| group.len() == length && is_hex(group))
}

fn is_mac(word: &str) -> bool {
    [':', '-'].iter().any(|separator| {
        let bytes: Vec<&str> = word.split(*separator).collect();
        bytes.len() == MAC_BYTES && bytes.iter().all(|byte| byte.len() == 2 && is_hex(byte))
    })
}

fn is_hex(text: &str) -> bool {
    text.chars().all(|character| character.is_ascii_hexdigit())
}

/// `/dev/serial/by-id/usb-Klipper_..._SERIAL-if00` becomes `/dev/serial/by-id/<redacted>`.
fn serial_path_redacted(word: &str) -> String {
    for folder in SERIAL_FOLDERS {
        if let Some(start) = word.find(folder) {
            return format!("{}{REDACTED}", &word[..start + folder.len()]);
        }
    }
    word.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn addresses_urls_uuids_and_macs_in_text_go() {
        let line = "connect ws://192.168.1.20:7125/websocket from 10.0.0.5:4711 to voron.local (aa:bb:cc:dd:ee:ff) \
                    id 123e4567-e89b-12d3-a456-426614174000 via fe80::1.\n";
        assert_eq!(text(line), "connect <url> from <address> to <address> (<mac>) id <uuid> via <address>.\n");
    }

    #[test]
    fn plain_text_and_versions_stay() {
        let line = "Klipper v0.12.0-300-gabc  ready, extruder 210.5 / 215 C\n";
        assert_eq!(text(line), line);
    }

    #[test]
    fn home_and_serial_paths_lose_the_private_part() {
        assert_eq!(path("/home/frank/printer_data/config/sailhand.conf"), "~/printer_data/config/sailhand.conf");
        assert_eq!(text("--settings /Users/anna/s.conf"), "--settings ~/s.conf");
        assert_eq!(text("/dev/serial/by-id/usb-Klipper_stm32f446xx_1A2B3C-if00"), "/dev/serial/by-id/<redacted>");
    }

    #[test]
    fn json_keys_and_strings_are_redacted_recursively() {
        let mut report = json!({
            "mcu": { "serial": "/dev/serial/by-id/x", "canbus_uuid": "abc123abc123", "baud": 250000 },
            "wled": { "address": "10.0.0.9" },
            "note": "see http://10.0.0.9/x",
            "list": [{ "hostname": "voron", "state": "ready" }],
        });
        value(&mut report);
        assert_eq!(report["mcu"], json!({ "serial": REDACTED, "canbus_uuid": REDACTED, "baud": 250000 }));
        assert_eq!(report["wled"]["address"], REDACTED);
        assert_eq!(report["note"], "see <url>");
        assert_eq!(report["list"], json!([{ "hostname": REDACTED, "state": "ready" }]));
    }

    #[test]
    fn the_command_line_hides_the_moonraker_address() {
        let arguments: Vec<String> =
            ["--fullscreen", "-m", "voron", "--settings", "/home/frank/a.conf", "--moonraker=printer", "-mx"]
                .map(str::to_owned)
                .to_vec();
        assert_eq!(
            command_line(&arguments),
            "--fullscreen -m <redacted> --settings ~/a.conf --moonraker=<redacted> -m<redacted>"
        );
    }
}
