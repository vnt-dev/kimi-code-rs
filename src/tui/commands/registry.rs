use std::{fs, path::PathBuf, sync::LazyLock};

use regex::Regex;

use super::{
    complete_args::{ArgCompletionSpec, complete_leading_arg},
    types::{AutocompleteItem, KimiSlashCommand, SlashCommandAvailability},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCommandAvailabilityRule {
    Default,
    Always,
    IdleOnly,
    Plan,
    Goal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashArgumentCompletionKind {
    Goal,
    Swarm,
    AddDir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinSlashCommand {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub priority: Option<i32>,
    pub availability: SlashCommandAvailabilityRule,
    pub argument_hint: Option<&'static str>,
    pub completion: Option<SlashArgumentCompletionKind>,
    pub experimental_flag: Option<&'static str>,
}

macro_rules! command {
    ($name:literal, [$($alias:literal),*], $description:literal, $priority:expr, $availability:ident) => {
        BuiltinSlashCommand {
            name: $name,
            aliases: &[$($alias),*],
            description: $description,
            priority: $priority,
            availability: SlashCommandAvailabilityRule::$availability,
            argument_hint: None,
            completion: None,
            experimental_flag: None,
        }
    };
}

pub const BUILTIN_SLASH_COMMANDS: &[BuiltinSlashCommand] = &[
    command!(
        "yolo",
        ["yes"],
        "Toggle YOLO mode: auto-approve tool actions, but the agent may still ask questions.",
        Some(101),
        Always
    ),
    command!(
        "auto",
        [],
        "Toggle Auto mode: fully autonomous, agent decides everything without asking.",
        Some(99),
        Always
    ),
    command!(
        "permission",
        [],
        "Select permission mode",
        Some(100),
        Always
    ),
    command!(
        "settings",
        ["config"],
        "Open TUI settings",
        Some(100),
        Always
    ),
    command!("plan", [], "Toggle plan mode", Some(100), Plan),
    BuiltinSlashCommand {
        name: "swarm",
        aliases: &[],
        description: "Toggle swarm mode or run one task in swarm mode",
        priority: Some(100),
        availability: SlashCommandAvailabilityRule::IdleOnly,
        argument_hint: Some("[on|off] | <task>"),
        completion: Some(SlashArgumentCompletionKind::Swarm),
        experimental_flag: None,
    },
    command!("model", [], "Switch LLM model", Some(100), Always),
    command!(
        "effort",
        ["thinking"],
        "Switch thinking effort",
        Some(95),
        Always
    ),
    command!(
        "provider",
        ["providers"],
        "Manage AI providers (add / delete / refresh)",
        Some(95),
        Always
    ),
    command!(
        "btw",
        [],
        "Ask a forked side agent a question",
        Some(90),
        Always
    ),
    command!(
        "help",
        ["h", "?"],
        "Show available commands and shortcuts",
        Some(80),
        Always
    ),
    command!(
        "new",
        ["clear"],
        "Start a fresh session in the current workspace",
        Some(80),
        Default
    ),
    command!(
        "sessions",
        ["resume"],
        "Browse and resume sessions",
        Some(80),
        Default
    ),
    command!(
        "tasks",
        ["task"],
        "Browse background tasks",
        Some(80),
        Always
    ),
    command!("mcp", [], "Show MCP server status", Some(60), Always),
    command!("plugins", [], "Manage plugins", Some(60), Always),
    BuiltinSlashCommand {
        name: "add-dir",
        aliases: &[],
        description: "Add or list an additional workspace directory",
        priority: Some(60),
        availability: SlashCommandAvailabilityRule::IdleOnly,
        argument_hint: Some("[list] | <path>"),
        completion: Some(SlashArgumentCompletionKind::AddDir),
        experimental_flag: None,
    },
    command!(
        "experiments",
        ["experimental"],
        "Manage experimental features",
        Some(60),
        IdleOnly
    ),
    command!(
        "reload",
        [],
        "Reload session and apply config.toml settings plus tui.toml UI preferences",
        Some(60),
        IdleOnly
    ),
    command!(
        "reload-tui",
        [],
        "Reload only tui.toml UI preferences",
        Some(60),
        Always
    ),
    BuiltinSlashCommand {
        name: "compact",
        aliases: &[],
        description: "Compact the conversation context",
        priority: Some(80),
        availability: SlashCommandAvailabilityRule::Default,
        argument_hint: Some("<instruction>"),
        completion: None,
        experimental_flag: None,
    },
    BuiltinSlashCommand {
        name: "goal",
        aliases: &[],
        description: "Start or manage an autonomous goal",
        priority: Some(80),
        availability: SlashCommandAvailabilityRule::Goal,
        argument_hint: Some("[status|pause|resume|cancel|replace|next] | <objective>"),
        completion: Some(SlashArgumentCompletionKind::Goal),
        experimental_flag: None,
    },
    command!(
        "init",
        [],
        "Analyze the codebase and generate AGENTS.md",
        None,
        Default
    ),
    command!("fork", [], "Fork the current session", Some(80), Default),
    BuiltinSlashCommand {
        name: "title",
        aliases: &["rename"],
        description: "Set or show session title",
        priority: Some(60),
        availability: SlashCommandAvailabilityRule::Always,
        argument_hint: Some("<title>"),
        completion: None,
        experimental_flag: None,
    },
    command!(
        "usage",
        [],
        "Show session tokens + context window + plan quotas",
        Some(60),
        Always
    ),
    command!(
        "status",
        [],
        "Show current session and runtime status",
        Some(60),
        Always
    ),
    command!(
        "feedback",
        [],
        "Send feedback to make Kimi Code better",
        Some(60),
        Always
    ),
    command!(
        "undo",
        [],
        "Withdraw the last prompt from the transcript",
        Some(80),
        IdleOnly
    ),
    command!(
        "editor",
        [],
        "Set the external editor for Ctrl-G",
        Some(60),
        Always
    ),
    command!("theme", [], "Set the terminal UI theme", Some(60), Always),
    command!(
        "logout",
        ["disconnect"],
        "Log out of a configured provider",
        Some(40),
        Default
    ),
    command!(
        "login",
        [],
        "Select a platform and authenticate",
        Some(40),
        Default
    ),
    command!(
        "export-md",
        ["export"],
        "Export current session as a Markdown file",
        Some(40),
        Default
    ),
    command!(
        "export-debug-zip",
        [],
        "Export current session as a debug ZIP archive",
        Some(40),
        Default
    ),
    command!(
        "copy",
        [],
        "Copy the last assistant message to the clipboard",
        Some(40),
        Default
    ),
    command!(
        "web",
        [],
        "Open the current session in the Web UI by starting a new server",
        Some(40),
        Always
    ),
    command!(
        "exit",
        ["quit", "q"],
        "Exit the application",
        Some(20),
        Default
    ),
    command!("version", [], "Show version information", Some(20), Always),
];

const GOAL_ARG_COMPLETIONS: &[(&str, &str)] = &[
    ("status", "Show the current goal"),
    ("pause", "Pause the active goal"),
    ("resume", "Resume a paused goal"),
    ("cancel", "Cancel and remove the current goal"),
    ("replace", "Replace the current goal with a new objective"),
    ("next", "Queue an upcoming goal"),
];
const GOAL_NEXT_ARG_COMPLETIONS: &[(&str, &str)] = &[("manage", "Manage upcoming goals")];
const SWARM_ARG_COMPLETIONS: &[(&str, &str)] =
    &[("on", "Turn swarm mode on"), ("off", "Turn swarm mode off")];
const ADD_DIR_ARG_COMPLETIONS: &[(&str, &str)] =
    &[(("list"), "Show configured additional workspace directories")];

fn completion_specs(values: &[(&str, &str)]) -> Vec<ArgCompletionSpec> {
    values
        .iter()
        .map(|(value, description)| ArgCompletionSpec {
            value: (*value).to_owned(),
            description: (*description).to_owned(),
        })
        .collect()
}

/// Original: registry.ts goalArgumentCompletions()
pub fn goal_argument_completions(argument_prefix: &str) -> Option<Vec<AutocompleteItem>> {
    static NEXT: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(?i)^next\s+(\S*)$").ok());
    if let Some(captures) = NEXT
        .as_ref()
        .and_then(|regex| regex.captures(argument_prefix))
    {
        let prefix = captures.get(1).map_or("", |capture| capture.as_str());
        return complete_leading_arg(&completion_specs(GOAL_NEXT_ARG_COMPLETIONS), prefix).map(
            |mut items| {
                for item in &mut items {
                    item.value = format!("next {}", item.value);
                }
                items
            },
        );
    }
    complete_leading_arg(&completion_specs(GOAL_ARG_COMPLETIONS), argument_prefix)
}

/// Original: registry.ts swarmArgumentCompletions()
pub fn swarm_argument_completions(argument_prefix: &str) -> Option<Vec<AutocompleteItem>> {
    complete_leading_arg(&completion_specs(SWARM_ARG_COMPLETIONS), argument_prefix)
}

/// Original: registry.ts addDirArgumentCompletions()
pub fn add_dir_argument_completions(argument_prefix: &str) -> Option<Vec<AutocompleteItem>> {
    if !is_path_like_add_dir_argument(argument_prefix) {
        return complete_leading_arg(&completion_specs(ADD_DIR_ARG_COMPLETIONS), argument_prefix);
    }
    complete_add_dir_path(argument_prefix)
}

fn is_path_like_add_dir_argument(value: &str) -> bool {
    value == "."
        || value == ".."
        || value.starts_with("./")
        || value.starts_with("../")
        || value.starts_with('/')
        || value.starts_with('~')
}

fn complete_add_dir_path(argument_prefix: &str) -> Option<Vec<AutocompleteItem>> {
    let normalized = if argument_prefix == "~" {
        "~/"
    } else {
        argument_prefix
    };
    let expanded = expand_home_prefix(normalized)?;
    let parent_input = if normalized == "/" || normalized.ends_with('/') {
        expanded.clone()
    } else {
        expanded.parent().map(PathBuf::from).unwrap_or_default()
    };
    let partial = if normalized.ends_with('/') {
        String::new()
    } else {
        expanded
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let entries = fs::read_dir(&parent_input).ok()?;
    let mut items = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.')
            || (!partial.is_empty() && !name.to_lowercase().starts_with(&partial.to_lowercase()))
            || !entry.path().is_dir()
        {
            continue;
        }
        let value = directory_completion_value(normalized, &parent_input, &name)?;
        items.push(AutocompleteItem {
            value,
            label: format!("{name}/"),
            description: path_text(entry.path()),
        });
    }
    (!items.is_empty()).then_some(items)
}

fn expand_home_prefix(value: &str) -> Option<PathBuf> {
    if value == "~" || value == "~/" {
        return dirs::home_dir();
    }
    if let Some(relative) = value.strip_prefix("~/") {
        return dirs::home_dir().map(|home| home.join(relative));
    }
    Some(PathBuf::from(value))
}

fn directory_completion_value(
    argument_prefix: &str,
    parent: &std::path::Path,
    entry_name: &str,
) -> Option<String> {
    if argument_prefix.starts_with("~/") {
        let home = dirs::home_dir()?;
        let relative = parent.strip_prefix(home).ok()?;
        let middle = path_text(relative);
        return Some(if middle.is_empty() {
            format!("~/{entry_name}/")
        } else {
            format!("~/{middle}/{entry_name}/")
        });
    }
    Some(format!("{}/", path_text(parent.join(entry_name))))
}

fn path_text(path: impl AsRef<std::path::Path>) -> String {
    path.as_ref().to_string_lossy().replace('\\', "/")
}

/// Original: registry.ts findBuiltInSlashCommand()
pub fn find_built_in_slash_command(command_name: &str) -> Option<&'static BuiltinSlashCommand> {
    BUILTIN_SLASH_COMMANDS
        .iter()
        .find(|command| command.name == command_name || command.aliases.contains(&command_name))
}

/// Original: registry.ts resolveSlashCommandAvailability()
pub fn resolve_slash_command_availability(
    command: &BuiltinSlashCommand,
    args: &str,
) -> SlashCommandAvailability {
    match command.availability {
        SlashCommandAvailabilityRule::Always => SlashCommandAvailability::Always,
        SlashCommandAvailabilityRule::Default | SlashCommandAvailabilityRule::IdleOnly => {
            SlashCommandAvailability::IdleOnly
        }
        SlashCommandAvailabilityRule::Plan => {
            if args.trim().eq_ignore_ascii_case("clear") {
                SlashCommandAvailability::IdleOnly
            } else {
                SlashCommandAvailability::Always
            }
        }
        SlashCommandAvailabilityRule::Goal => match args.trim() {
            "" | "status" | "pause" | "cancel" | "next" => SlashCommandAvailability::Always,
            value if value.starts_with("next ") => SlashCommandAvailability::Always,
            _ => SlashCommandAvailability::IdleOnly,
        },
    }
}

/// Original: registry.ts sortSlashCommands()
pub fn sort_slash_commands(commands: &[KimiSlashCommand]) -> Vec<KimiSlashCommand> {
    let mut sorted = commands.to_vec();
    sorted.sort_by(|left, right| {
        right
            .priority
            .unwrap_or_default()
            .cmp(&left.priority.unwrap_or_default())
            .then_with(|| left.name.cmp(&right.name))
    });
    sorted
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn finds_commands_by_name_and_alias() {
        assert_eq!(
            find_built_in_slash_command("exit").map(|c| c.name),
            Some("exit")
        );
        assert_eq!(
            find_built_in_slash_command("q").map(|c| c.name),
            Some("exit")
        );
        assert_eq!(
            find_built_in_slash_command("clear").map(|c| c.name),
            Some("new")
        );
        assert!(find_built_in_slash_command("unknown").is_none());
        let names = BUILTIN_SLASH_COMMANDS
            .iter()
            .map(|c| c.name)
            .collect::<HashSet<_>>();
        assert_eq!(names.len(), BUILTIN_SLASH_COMMANDS.len());
    }

    #[test]
    fn resolves_dynamic_and_default_availability() {
        let plan = find_built_in_slash_command("plan").expect("plan is registered");
        assert_eq!(
            resolve_slash_command_availability(plan, "on"),
            SlashCommandAvailability::Always
        );
        assert_eq!(
            resolve_slash_command_availability(plan, "clear"),
            SlashCommandAvailability::IdleOnly
        );
        let goal = find_built_in_slash_command("goal").expect("goal is registered");
        for args in ["", "status", "pause", "cancel", "next", "next manage"] {
            assert_eq!(
                resolve_slash_command_availability(goal, args),
                SlashCommandAvailability::Always
            );
        }
        for args in ["resume", "clear", "Ship feature X", "status report"] {
            assert_eq!(
                resolve_slash_command_availability(goal, args),
                SlashCommandAvailability::IdleOnly
            );
        }
        let init = find_built_in_slash_command("init").expect("init is registered");
        assert_eq!(
            resolve_slash_command_availability(init, ""),
            SlashCommandAvailability::IdleOnly
        );
    }

    #[test]
    fn completes_goal_and_swarm_subcommands() {
        assert_eq!(
            goal_argument_completions("next m")
                .unwrap_or_default()
                .into_iter()
                .map(|item| item.value)
                .collect::<Vec<_>>(),
            ["next manage"]
        );
        assert_eq!(
            swarm_argument_completions("O")
                .unwrap_or_default()
                .into_iter()
                .map(|item| item.value)
                .collect::<Vec<_>>(),
            ["on", "off"]
        );
        assert_eq!(swarm_argument_completions("on"), None);
        assert_eq!(
            add_dir_argument_completions("L")
                .unwrap_or_default()
                .into_iter()
                .map(|item| item.value)
                .collect::<Vec<_>>(),
            ["list"]
        );
    }

    #[test]
    fn sorts_priority_descending_then_name_ascending() {
        let command = |name: &str, priority| KimiSlashCommand {
            name: name.to_owned(),
            aliases: vec![],
            description: name.to_owned(),
            priority,
            availability: None,
            experimental_flag: None,
        };
        let sorted = sort_slash_commands(&[
            command("zebra", Some(100)),
            command("alpha", Some(100)),
            command("middle", Some(50)),
            command("plain", None),
        ]);
        assert_eq!(
            sorted.into_iter().map(|c| c.name).collect::<Vec<_>>(),
            ["alpha", "zebra", "middle", "plain"]
        );
    }
}
