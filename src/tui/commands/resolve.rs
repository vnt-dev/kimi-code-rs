use std::collections::{BTreeMap, HashMap};

use indexmap::IndexMap;

use super::{
    experimental_flags::is_experimental_flag_enabled,
    parse::parse_slash_input,
    registry::{
        BuiltinSlashCommand, find_built_in_slash_command, resolve_slash_command_availability,
    },
    types::{SlashCommandAvailability, SlashCommandBusyReason},
};

pub trait CommandLookup {
    fn value(&self, command_name: &str) -> Option<&str>;

    fn contains(&self, command_name: &str) -> bool {
        self.value(command_name).is_some()
    }
}

impl CommandLookup for HashMap<String, String> {
    fn value(&self, command_name: &str) -> Option<&str> {
        self.get(command_name).map(String::as_str)
    }
}

impl CommandLookup for BTreeMap<String, String> {
    fn value(&self, command_name: &str) -> Option<&str> {
        self.get(command_name).map(String::as_str)
    }
}

impl CommandLookup for IndexMap<String, String> {
    fn value(&self, command_name: &str) -> Option<&str> {
        self.get(command_name).map(String::as_str)
    }
}

impl CommandLookup for Vec<(String, String)> {
    fn value(&self, command_name: &str) -> Option<&str> {
        self.iter()
            .find_map(|(command, value)| (command == command_name).then_some(value.as_str()))
    }
}

pub struct ResolveSlashCommandInput<'a> {
    pub input: &'a str,
    pub skill_command_map: &'a dyn CommandLookup,
    pub plugin_command_map: &'a dyn CommandLookup,
    pub is_streaming: bool,
    pub is_compacting: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommandIntent {
    NotCommand,
    Builtin {
        command: &'static BuiltinSlashCommand,
        name: &'static str,
        args: String,
    },
    Skill {
        command_name: String,
        skill_name: String,
        args: String,
    },
    PluginCommand {
        command_name: String,
        plugin_id: String,
        args: String,
    },
    Message {
        input: String,
    },
    Blocked {
        command_name: String,
        reason: SlashCommandBusyReason,
    },
}

/// Original:
///   apps/kimi-code/src/tui/commands/resolve.ts
///   resolveSlashCommandInput()
pub fn resolve_slash_command_input(options: ResolveSlashCommandInput<'_>) -> SlashCommandIntent {
    let Some(parsed) = parse_slash_input(options.input) else {
        return SlashCommandIntent::NotCommand;
    };

    if let Some(command) = find_built_in_slash_command(&parsed.name)
        && is_experimental_flag_enabled(command.experimental_flag)
    {
        if let Some(reason) = slash_command_busy_reason(options.is_streaming, options.is_compacting)
            && resolve_slash_command_availability(command, &parsed.args)
                == SlashCommandAvailability::IdleOnly
        {
            return SlashCommandIntent::Blocked {
                command_name: parsed.name,
                reason,
            };
        }
        return SlashCommandIntent::Builtin {
            command,
            name: command.name,
            args: parsed.args,
        };
    }

    if let Some(skill_name) = resolve_skill_command(options.skill_command_map, &parsed.name) {
        if let Some(reason) = slash_command_busy_reason(options.is_streaming, options.is_compacting)
        {
            return SlashCommandIntent::Blocked {
                command_name: parsed.name,
                reason,
            };
        }
        return SlashCommandIntent::Skill {
            command_name: parsed.name,
            skill_name: skill_name.to_owned(),
            args: parsed.args.trim().to_owned(),
        };
    }

    if options.plugin_command_map.contains(&parsed.name) {
        if let Some(reason) = slash_command_busy_reason(options.is_streaming, options.is_compacting)
        {
            return SlashCommandIntent::Blocked {
                command_name: parsed.name,
                reason,
            };
        }
        let (plugin_id, command_name) = parsed
            .name
            .split_once(':')
            .map_or((parsed.name.as_str(), ""), |parts| parts);
        return SlashCommandIntent::PluginCommand {
            command_name: command_name.to_owned(),
            plugin_id: plugin_id.to_owned(),
            args: parsed.args.trim().to_owned(),
        };
    }

    SlashCommandIntent::Message {
        input: options.input.to_owned(),
    }
}

/// Original: resolve.ts resolveSkillCommand()
pub fn resolve_skill_command<'a>(
    skill_command_map: &'a dyn CommandLookup,
    command_name: &str,
) -> Option<&'a str> {
    skill_command_map
        .value(command_name)
        .or_else(|| skill_command_map.value(&format!("skill:{command_name}")))
}

/// Original: resolve.ts slashCommandBusyReason()
pub const fn slash_command_busy_reason(
    is_streaming: bool,
    is_compacting: bool,
) -> Option<SlashCommandBusyReason> {
    if is_streaming {
        Some(SlashCommandBusyReason::Streaming)
    } else if is_compacting {
        Some(SlashCommandBusyReason::Compacting)
    } else {
        None
    }
}

/// Original: resolve.ts slashBusyMessage()
pub fn slash_busy_message(command_name: &str, reason: SlashCommandBusyReason) -> String {
    match reason {
        SlashCommandBusyReason::Streaming => {
            format!("Cannot /{command_name} while streaming — press Esc or Ctrl-C first.")
        }
        SlashCommandBusyReason::Compacting => format!(
            "Cannot /{command_name} while compacting — wait for compaction to finish first."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(
        input: &str,
        skills: &dyn CommandLookup,
        plugins: &dyn CommandLookup,
        is_streaming: bool,
        is_compacting: bool,
    ) -> SlashCommandIntent {
        resolve_slash_command_input(ResolveSlashCommandInput {
            input,
            skill_command_map: skills,
            plugin_command_map: plugins,
            is_streaming,
            is_compacting,
        })
    }

    #[test]
    fn distinguishes_plain_text_builtin_alias_and_unknown_slash_input() {
        let empty = HashMap::new();
        assert_eq!(
            resolve("hello", &empty, &empty, false, false),
            SlashCommandIntent::NotCommand
        );
        assert!(matches!(
            resolve("/q", &empty, &empty, false, false),
            SlashCommandIntent::Builtin { name: "exit", ref args, .. } if args.is_empty()
        ));
        assert_eq!(
            resolve("/missing arg", &empty, &empty, false, false),
            SlashCommandIntent::Message {
                input: "/missing arg".to_owned()
            }
        );
    }

    #[test]
    fn blocks_only_idle_only_builtins_while_busy() {
        let empty = HashMap::new();
        assert_eq!(
            resolve("/new", &empty, &empty, true, false),
            SlashCommandIntent::Blocked {
                command_name: "new".to_owned(),
                reason: SlashCommandBusyReason::Streaming,
            }
        );
        assert!(matches!(
            resolve("/mcp", &empty, &empty, true, false),
            SlashCommandIntent::Builtin { name: "mcp", .. }
        ));
        assert!(matches!(
            resolve("/goal status", &empty, &empty, true, false),
            SlashCommandIntent::Builtin { name: "goal", .. }
        ));
        assert!(matches!(
            resolve("/goal create it", &empty, &empty, true, false),
            SlashCommandIntent::Blocked {
                reason: SlashCommandBusyReason::Streaming,
                ..
            }
        ));
    }

    #[test]
    fn resolves_prefixed_and_unprefixed_skill_commands() {
        let skills = HashMap::from([
            ("skill:review".to_owned(), "review".to_owned()),
            ("mcp-config".to_owned(), "mcp-config".to_owned()),
        ]);
        let empty = HashMap::new();
        assert_eq!(resolve_skill_command(&skills, "review"), Some("review"));
        assert_eq!(
            resolve("/skill:review src", &skills, &empty, false, false),
            SlashCommandIntent::Skill {
                command_name: "skill:review".to_owned(),
                skill_name: "review".to_owned(),
                args: "src".to_owned(),
            }
        );
        assert!(matches!(
            resolve("/mcp-config", &skills, &empty, false, true),
            SlashCommandIntent::Blocked {
                reason: SlashCommandBusyReason::Compacting,
                ..
            }
        ));
    }

    #[test]
    fn resolves_namespaced_and_nested_plugin_commands() {
        let empty = HashMap::new();
        let plugins =
            IndexMap::from([("my-plugin:frontend/component".to_owned(), "body".to_owned())]);
        assert_eq!(
            resolve(
                "/my-plugin:frontend/component spin",
                &empty,
                &plugins,
                false,
                false
            ),
            SlashCommandIntent::PluginCommand {
                command_name: "frontend/component".to_owned(),
                plugin_id: "my-plugin".to_owned(),
                args: "spin".to_owned(),
            }
        );
        assert!(matches!(
            resolve(
                "/my-plugin:frontend/component",
                &empty,
                &plugins,
                true,
                false
            ),
            SlashCommandIntent::Blocked {
                reason: SlashCommandBusyReason::Streaming,
                ..
            }
        ));
    }

    #[test]
    fn streaming_takes_precedence_and_busy_messages_match_reason() {
        assert_eq!(
            slash_command_busy_reason(true, true),
            Some(SlashCommandBusyReason::Streaming)
        );
        assert!(
            slash_busy_message("new", SlashCommandBusyReason::Streaming)
                .contains("Cannot /new while streaming")
        );
        assert!(
            slash_busy_message("new", SlashCommandBusyReason::Compacting)
                .contains("Cannot /new while compacting")
        );
    }
}
