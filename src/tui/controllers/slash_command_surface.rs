#[cfg(test)]
use crate::tui::commands::registry::BUILTIN_SLASH_COMMANDS;
use crate::tui::commands::{parse::parse_slash_input, registry::find_built_in_slash_command};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommandSurfaceAction {
    Empty,
    Exit,
    ClearTranscript,
    ShowVersion(String),
    ShowHelp,
    Pending {
        command_name: &'static str,
        args: String,
    },
    Unknown(String),
}

// Original:
//   apps/kimi-code/src/tui/commands/dispatch.ts
//   executeSlashCommand()
//
// Rust adaptation:
//   This synchronous surface resolver handles the actions that can be
//   completed without an agent session and acknowledges every other built-in.
//   The async command dispatcher remains the method-level counterpart used
//   once KimiTui is composed with kimi-code-agent-core-v2 services.
pub fn resolve_slash_command_surface(input: &str, version: &str) -> SlashCommandSurfaceAction {
    let Some(parsed) = parse_slash_input(input) else {
        return if input.trim_matches(['/', ' ']).is_empty() {
            SlashCommandSurfaceAction::Empty
        } else {
            SlashCommandSurfaceAction::Unknown(input.trim().to_owned())
        };
    };
    let Some(command) = find_built_in_slash_command(&parsed.name) else {
        return SlashCommandSurfaceAction::Unknown(parsed.name);
    };

    match command.name {
        "exit" => SlashCommandSurfaceAction::Exit,
        "help" => SlashCommandSurfaceAction::ShowHelp,
        "version" => SlashCommandSurfaceAction::ShowVersion(format!("Kimi Code v{version}")),
        "new" => SlashCommandSurfaceAction::ClearTranscript,
        command_name => SlashCommandSurfaceAction::Pending {
            command_name,
            args: parsed.args,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_immediate_commands_and_aliases() {
        assert_eq!(
            resolve_slash_command_surface("/q", "1.2.3"),
            SlashCommandSurfaceAction::Exit
        );
        assert_eq!(
            resolve_slash_command_surface("/clear", "1.2.3"),
            SlashCommandSurfaceAction::ClearTranscript
        );
        assert_eq!(
            resolve_slash_command_surface("/version", "1.2.3"),
            SlashCommandSurfaceAction::ShowVersion("Kimi Code v1.2.3".to_owned())
        );
    }

    #[test]
    fn acknowledges_every_registered_command() {
        for command in BUILTIN_SLASH_COMMANDS {
            let action = resolve_slash_command_surface(&format!("/{}", command.name), "1.2.3");
            assert!(
                !matches!(
                    action,
                    SlashCommandSurfaceAction::Empty | SlashCommandSurfaceAction::Unknown(_)
                ),
                "{} was not routed",
                command.name
            );
        }
    }

    #[test]
    fn preserves_pending_arguments_and_rejects_unknown_commands() {
        assert_eq!(
            resolve_slash_command_surface("/goal  ship it ", "1.2.3"),
            SlashCommandSurfaceAction::Pending {
                command_name: "goal",
                args: "ship it".to_owned(),
            }
        );
        assert_eq!(
            resolve_slash_command_surface("/missing arg", "1.2.3"),
            SlashCommandSurfaceAction::Unknown("missing".to_owned())
        );
        assert_eq!(
            resolve_slash_command_surface("/", "1.2.3"),
            SlashCommandSurfaceAction::Empty
        );
    }
}
