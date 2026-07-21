use indexmap::IndexMap;

use crate::sdk::types::PluginCommandDef;

use super::types::KimiSlashCommand;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSlashCommands {
    pub commands: Vec<KimiSlashCommand>,
    pub command_map: IndexMap<String, String>,
}

pub fn plugin_command_name(plugin_id: &str, name: &str) -> String {
    format!("{plugin_id}:{name}")
}

/// Original:
///   apps/kimi-code/src/tui/commands/plugin-commands.ts
///   buildPluginSlashCommands()
pub fn build_plugin_slash_commands(definitions: &[PluginCommandDef]) -> PluginSlashCommands {
    let mut command_map = IndexMap::new();
    let commands = definitions
        .iter()
        .map(|definition| {
            let command_name = plugin_command_name(&definition.plugin_id, &definition.name);
            command_map.insert(command_name.clone(), definition.body.clone());
            KimiSlashCommand {
                name: command_name,
                aliases: Vec::new(),
                description: definition.description.clone(),
                priority: None,
                availability: None,
                experimental_flag: None,
            }
        })
        .collect();
    PluginSlashCommands {
        commands,
        command_map,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_commands_and_maps_bodies_in_source_order() {
        let definitions = [
            PluginCommandDef {
                plugin_id: "my-plugin".to_owned(),
                name: "deploy".to_owned(),
                description: "Deploy".to_owned(),
                body: "Deploy $ARGUMENTS".to_owned(),
                path: "/p/deploy.md".to_owned(),
            },
            PluginCommandDef {
                plugin_id: "other".to_owned(),
                name: "check".to_owned(),
                description: "Check".to_owned(),
                body: "Check now".to_owned(),
                path: "/p/check.md".to_owned(),
            },
        ];
        let result = build_plugin_slash_commands(&definitions);

        assert_eq!(
            plugin_command_name("my-plugin", "deploy"),
            "my-plugin:deploy"
        );
        assert_eq!(
            result
                .commands
                .iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>(),
            ["my-plugin:deploy", "other:check"]
        );
        assert_eq!(
            result
                .command_map
                .get("my-plugin:deploy")
                .map(String::as_str),
            Some("Deploy $ARGUMENTS")
        );
    }

    #[test]
    fn returns_empty_collections_for_no_definitions() {
        let result = build_plugin_slash_commands(&[]);
        assert!(result.commands.is_empty());
        assert!(result.command_map.is_empty());
    }
}
