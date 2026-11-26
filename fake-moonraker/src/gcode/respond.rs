//! `RESPOND` and the fake-only demo macros that send macro prompts (`action:prompt_*`).

use super::*;

/// Klipper's `extras/respond.py`: `RESPOND [TYPE=echo|command|error] [PREFIX=<text>] [MSG=<text>]` answers with
/// `<prefix> <message>`; the type chooses the prefix (`echo:`, `//`, `!!`), `echo` is the default.
/// `TYPE=command MSG="action:prompt_begin ..."` is how macros ask a client such as Mainsail to show a dialog.
pub(super) fn parse(line: &str, name: &str, args: &[&str]) -> Result<Option<Command>, CommandError> {
    let command = match name {
        "RESPOND" => respond(line)?,
        DEMO_BED_CLEAR_PROMPT => Command::Respond(BED_CLEAR_PROMPT.iter().map(|line| prompt_line(line)).collect()),
        DEMO_BUTTONS_PROMPT => Command::Respond(BUTTONS_PROMPT.iter().map(|line| prompt_line(line)).collect()),
        // What the buttons of the demo prompts send: the macro that was asked answers by ending the prompt, as a
        // well-behaved macro does (Mainsail only closes its dialog on `prompt_end`).
        "BED_CLEAR" | DEMO_PROMPT_ANSWER => {
            let answer = args.iter().find(|arg| arg.contains("CANCEL=1")).map_or("bed is clear", |_| "cancelled");
            Command::Respond(vec![prompt_line("end"), format!("// Demo prompt answered: {answer}")])
        }
        _ => return Ok(None),
    };
    Ok(Some(command))
}

/// The demo macros, listed with the printer's macros.
pub(crate) const DEMO_BED_CLEAR_PROMPT: &str = "DEMO_BED_CLEAR_PROMPT";
pub(crate) const DEMO_BUTTONS_PROMPT: &str = "DEMO_BUTTONS_PROMPT";
const DEMO_PROMPT_ANSWER: &str = "DEMO_PROMPT_ANSWER";

/// The example of the issue that asked for prompts: a question with two footer buttons.
const BED_CLEAR_PROMPT: [&str; 5] = [
    "begin Is the bed empty?",
    "text Homing Z raises the bed to the top. Anything on it gets crushed against the head.",
    "footer_button NO|BED_CLEAR CANCEL=1|secondary",
    "footer_button YES, EMPTY|BED_CLEAR|primary",
    "show",
];

/// Text, buttons in a row of their own, a button group and footer buttons, one of each style.
const BUTTONS_PROMPT: [&str; 14] = [
    "begin Filament runout",
    "text The filament ran out in tool T1.",
    "text Load new filament and choose how to go on.",
    "button_group_start",
    "button Load|DEMO_PROMPT_ANSWER|info",
    "button Unload|DEMO_PROMPT_ANSWER|warning",
    "button Purge|DEMO_PROMPT_ANSWER|secondary",
    "button_group_end",
    "button Retry|DEMO_PROMPT_ANSWER|primary",
    "button Abort print|DEMO_PROMPT_ANSWER|error",
    "button Plain, no style",
    "footer_button Later|DEMO_PROMPT_ANSWER|secondary",
    "footer_button Resume|DEMO_PROMPT_ANSWER|primary",
    "show",
];

fn prompt_line(text: &str) -> String {
    format!("// action:prompt_{text}")
}

fn respond(line: &str) -> Result<Command, CommandError> {
    let rest = line.split_once(char::is_whitespace).map_or("", |(_, rest)| rest);
    let params = parameters(rest);
    let find = |key: &str| params.iter().find(|(name, _)| name == key).map(|(_, value)| value.as_str());
    let default_prefix = "echo:";
    let prefix = match find("TYPE").map(str::to_ascii_lowercase).as_deref() {
        None => default_prefix,
        Some("echo") => "echo:",
        Some("command") => "//",
        Some("error") => "!!",
        Some(other) => {
            return Err(CommandError::new(format!(
                "RESPOND TYPE '{other}' is invalid. Must be one of 'echo', 'command', or 'error'"
            )));
        }
    };
    let prefix = find("PREFIX").unwrap_or(prefix);
    Ok(Command::Respond(vec![format!("{prefix} {}", find("MSG").unwrap_or_default())]))
}

/// `KEY=value` words as Klipper's G-code parser reads them: the key in upper case, a value in double quotes keeps
/// its spaces.
fn parameters(text: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut rest = text.trim_start();
    while let Some((key, after_key)) = rest.split_once('=') {
        let key = key.rsplit(char::is_whitespace).next().unwrap_or_default().to_ascii_uppercase();
        let (value, after_value) = match after_key.strip_prefix('"') {
            Some(quoted) => quoted.split_once('"').unwrap_or((quoted, "")),
            None => after_key.split_once(char::is_whitespace).unwrap_or((after_key, "")),
        };
        found.push((key, value.to_owned()));
        rest = after_value.trim_start();
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn responses(script: &str) -> Vec<String> {
        match parse_script(script).unwrap().as_slice() {
            [Command::Respond(lines)] => lines.clone(),
            other => panic!("not a response: {other:?}"),
        }
    }

    #[test]
    fn respond_command_prefixes_two_slashes_and_a_space() {
        assert_eq!(
            responses(r#"RESPOND TYPE=command MSG="action:prompt_footer_button NO|BED_CLEAR CANCEL=1|secondary""#),
            ["// action:prompt_footer_button NO|BED_CLEAR CANCEL=1|secondary"]
        );
        assert_eq!(responses(r#"RESPOND type="command" msg="action:prompt_end""#), ["// action:prompt_end"]);
    }

    #[test]
    fn respond_defaults_to_echo_and_knows_error() {
        assert_eq!(responses("RESPOND MSG=hello"), ["echo: hello"]);
        assert_eq!(responses("RESPOND TYPE=error MSG=\"went wrong\""), ["!! went wrong"]);
    }

    #[test]
    fn respond_rejects_an_unknown_type() {
        assert!(parse_script("RESPOND TYPE=shout MSG=x").is_err());
    }

    #[test]
    fn the_bed_clear_demo_is_the_prompt_of_the_issue() {
        let lines = responses(DEMO_BED_CLEAR_PROMPT);
        assert_eq!(lines.first().map(String::as_str), Some("// action:prompt_begin Is the bed empty?"));
        assert_eq!(lines.last().map(String::as_str), Some("// action:prompt_show"));
    }

    #[test]
    fn the_answer_ends_the_prompt() {
        assert_eq!(responses("BED_CLEAR CANCEL=1")[0], "// action:prompt_end");
    }
}
