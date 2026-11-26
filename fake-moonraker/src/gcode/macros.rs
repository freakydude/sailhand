//! Print-related macros and fake-only commands.

use super::*;

/// Parses `name` if it belongs to this group.
pub(super) fn parse(name: &str, args: &[&str]) -> Result<Option<Command>, CommandError> {
    let command: Result<Command, CommandError> = match name {
        "EXCLUDE_OBJECT" => Ok(Command::ExcludeObject { name: named_text(args, "NAME").unwrap_or_default() }),
        "SAVE_CONFIG" => Ok(Command::SaveConfig),
        "HELP" => Ok(Command::Help),
        "SET_PAUSE_NEXT_LAYER" => Ok(Command::LayerPause(LayerPauseCommand::NextLayer {
            enable: named_index(args, "ENABLE")?.is_none_or(|enable| enable != 0),
        })),
        "SET_PAUSE_AT_LAYER" => Ok(Command::LayerPause(LayerPauseCommand::AtLayer {
            enable: named_index(args, "ENABLE")?.map(|enable| enable != 0),
            layer: named_index(args, "LAYER")?.map(|layer| layer as u32),
        })),
        "FAKE_PRINT_ERROR" => Ok(Command::FakePrintError),
        "FAKE_MMU_PAUSE" => Ok(Command::FakeMmuPause {
            message: named_index(args, "MSG")?.is_some_and(|value| value != 0),
            extra_button: named_index(args, "BUTTONS")?.is_some_and(|value| value != 0),
        }),
        _ => return Ok(None),
    };
    command.map(Some)
}
