//! Skill activation prompt rendering.
//!
//! Original: `packages/agent-core-v2/src/agent/skill/prompt.ts`.

use crate::{_base::utils::xml_escape::escape_xml, app::skill_catalog::SkillSource};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkillPromptTrigger {
    UserSlash,
    ModelTool,
    NestedSkill,
}

impl SkillPromptTrigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::UserSlash => "user-slash",
            Self::ModelTool => "model-tool",
            Self::NestedSkill => "nested-skill",
        }
    }
}

pub struct RenderSkillPromptInput<'a> {
    pub skill_name: &'a str,
    pub skill_args: &'a str,
    pub skill_content: &'a str,
    pub skill_source: Option<SkillSource>,
    pub skill_dir: Option<&'a str>,
}

pub fn render_user_slash_skill_prompt(input: RenderSkillPromptInput<'_>) -> String {
    format!(
        "User activated the skill \"{}\". Follow the loaded skill instructions.\n\n{}",
        escape_xml(input.skill_name),
        render_skill_loaded_block(&input, SkillPromptTrigger::UserSlash)
    )
}

pub fn render_model_tool_skill_prompt(
    input: RenderSkillPromptInput<'_>,
    trigger: SkillPromptTrigger,
) -> String {
    debug_assert!(matches!(
        trigger,
        SkillPromptTrigger::ModelTool | SkillPromptTrigger::NestedSkill
    ));
    format!(
        "Skill tool loaded instructions for this request. Follow them.\n\n{}",
        render_skill_loaded_block(&input, trigger)
    )
}

pub fn render_skill_loaded_block(
    input: &RenderSkillPromptInput<'_>,
    trigger: SkillPromptTrigger,
) -> String {
    let attributes = [
        ("name", Some(input.skill_name)),
        ("trigger", Some(trigger.as_str())),
        ("source", input.skill_source.map(skill_source_name)),
        ("dir", input.skill_dir),
        ("args", Some(input.skill_args)),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|value| format!(" {name}=\"{}\"", escape_xml(value))))
    .collect::<String>();
    format!(
        "<kimi-skill-loaded{attributes}>\n{}\n</kimi-skill-loaded>",
        input.skill_content
    )
}

fn skill_source_name(source: SkillSource) -> &'static str {
    match source {
        SkillSource::Project => "project",
        SkillSource::User => "user",
        SkillSource::Extra => "extra",
        SkillSource::Builtin => "builtin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_slash_prompt_preserves_text_order_and_escapes_attributes() {
        assert_eq!(
            render_user_slash_skill_prompt(RenderSkillPromptInput {
                skill_name: "review<&",
                skill_args: "--path=\"a&b\"",
                skill_content: "Follow <raw> instructions.",
                skill_source: Some(SkillSource::Project),
                skill_dir: Some("/repo/a&b"),
            }),
            concat!(
                "User activated the skill \"review&lt;&amp;\". Follow the loaded skill instructions.\n\n",
                "<kimi-skill-loaded name=\"review&lt;&amp;\" trigger=\"user-slash\" source=\"project\" ",
                "dir=\"/repo/a&amp;b\" args=\"--path=&quot;a&amp;b&quot;\">\n",
                "Follow <raw> instructions.\n",
                "</kimi-skill-loaded>"
            )
        );
    }

    #[test]
    fn optional_attributes_are_omitted_but_empty_args_are_retained() {
        assert_eq!(
            render_model_tool_skill_prompt(
                RenderSkillPromptInput {
                    skill_name: "review",
                    skill_args: "",
                    skill_content: "body",
                    skill_source: None,
                    skill_dir: None,
                },
                SkillPromptTrigger::NestedSkill,
            ),
            concat!(
                "Skill tool loaded instructions for this request. Follow them.\n\n",
                "<kimi-skill-loaded name=\"review\" trigger=\"nested-skill\" args=\"\">\n",
                "body\n",
                "</kimi-skill-loaded>"
            )
        );
    }
}
