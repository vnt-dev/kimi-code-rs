use crate::sdk::types::{SkillSource, SkillSummary};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiSlashCommand {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSlashCommands {
    pub commands: Vec<KimiSlashCommand>,
    pub command_map: Vec<(String, String)>,
}

impl SkillSlashCommands {
    pub fn skill_name_for_command(&self, command_name: &str) -> Option<&str> {
        self.command_map
            .iter()
            .find_map(|(command, skill)| (command == command_name).then_some(skill.as_str()))
    }
}

// Original:
//   apps/kimi-code/src/tui/commands/skills.ts
//   isUserActivatableSkill()
pub fn is_user_activatable_skill(skill: &SkillSummary) -> bool {
    matches!(
        skill.skill_type.as_deref(),
        None | Some("prompt") | Some("inline") | Some("flow")
    )
}

// Original:
//   apps/kimi-code/src/tui/commands/skills.ts
//   buildSkillSlashCommands()
pub fn build_skill_slash_commands(skills: &[SkillSummary]) -> SkillSlashCommands {
    let mut sorted = skills.to_vec();
    sorted.sort_by(|left, right| {
        skill_group(left)
            .cmp(&skill_group(right))
            .then_with(|| left.name.cmp(&right.name))
    });

    let mut commands = Vec::new();
    let mut command_map = Vec::new();
    for skill in sorted
        .iter()
        .filter(|skill| is_user_activatable_skill(skill))
    {
        let command_name =
            if skill.source == Some(SkillSource::Builtin) || skill.is_sub_skill == Some(true) {
                skill.name.clone()
            } else {
                format!("skill:{}", skill.name)
            };
        command_map.push((command_name.clone(), skill.name.clone()));
        commands.push(KimiSlashCommand {
            name: command_name,
            aliases: Vec::new(),
            description: skill.description.clone().unwrap_or_default(),
        });
    }
    SkillSlashCommands {
        commands,
        command_map,
    }
}

fn skill_group(skill: &SkillSummary) -> u8 {
    u8::from(skill.source != Some(SkillSource::Builtin))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, skill_type: Option<&str>, source: Option<SkillSource>) -> SkillSummary {
        SkillSummary {
            name: name.to_owned(),
            description: Some(format!("{name} skill")),
            path: None,
            source,
            skill_type: skill_type.map(str::to_owned),
            disable_model_invocation: None,
            is_sub_skill: None,
        }
    }

    #[test]
    fn allows_only_user_activatable_skill_types() {
        for skill_type in [None, Some("prompt"), Some("inline"), Some("flow")] {
            assert!(is_user_activatable_skill(&skill(
                "allowed", skill_type, None
            )));
        }
        assert!(!is_user_activatable_skill(&skill(
            "agent-only",
            Some("agent"),
            None
        )));
    }

    #[test]
    fn builds_prefixed_external_commands_and_filters_agent_skills() {
        let mut nested = skill("nested-review", Some("prompt"), None);
        nested.description = Some("Nested review skill".to_owned());
        let built = build_skill_slash_commands(&[
            skill("review", Some("prompt"), None),
            nested,
            skill("agent-only", Some("agent"), None),
            skill("commit", Some("flow"), None),
        ]);
        assert_eq!(
            built
                .commands
                .iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>(),
            ["skill:commit", "skill:nested-review", "skill:review"]
        );
        assert_eq!(
            built.command_map,
            [
                ("skill:commit".to_owned(), "commit".to_owned()),
                ("skill:nested-review".to_owned(), "nested-review".to_owned()),
                ("skill:review".to_owned(), "review".to_owned()),
            ]
        );
        assert_eq!(
            built.commands[1].description,
            "Nested review skill".to_owned()
        );
    }

    #[test]
    fn sorts_builtins_before_external_skills_and_keeps_map_in_lockstep() {
        let built = build_skill_slash_commands(&[
            skill("zeta", Some("prompt"), Some(SkillSource::User)),
            skill("alpha", Some("prompt"), Some(SkillSource::Project)),
            skill("update-config", Some("inline"), Some(SkillSource::Builtin)),
            skill("mcp-config", Some("inline"), Some(SkillSource::Builtin)),
        ]);
        assert_eq!(
            built
                .commands
                .iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>(),
            ["mcp-config", "update-config", "skill:alpha", "skill:zeta"]
        );
        assert_eq!(
            built.skill_name_for_command("mcp-config"),
            Some("mcp-config")
        );
        assert_eq!(built.skill_name_for_command("skill:alpha"), Some("alpha"));
    }

    #[test]
    fn subskills_remain_unprefixed_and_disable_model_invocation_is_ignored() {
        let mut subskill = skill("outer.inner", Some("prompt"), Some(SkillSource::Project));
        subskill.is_sub_skill = Some(true);
        let mut builtin = skill("mcp-config", Some("inline"), Some(SkillSource::Builtin));
        builtin.disable_model_invocation = Some(true);
        let built = build_skill_slash_commands(&[subskill, builtin]);
        assert_eq!(
            built
                .commands
                .iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>(),
            ["mcp-config", "outer.inner"]
        );
    }
}
