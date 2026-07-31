//! Skill catalog models and type-policy helpers.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/types.ts`.

use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SkillSource {
    Project,
    User,
    Extra,
    Builtin,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SkillMetadata {
    pub name: Option<String>,
    pub description: Option<String>,
    pub kind: Option<String>,
    pub when_to_use: Option<String>,
    pub disable_model_invocation: Option<bool>,
    pub is_sub_skill: Option<bool>,
    pub safe: Option<bool>,
    pub arguments: Option<Value>,
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillPluginContext {
    pub id: String,
    pub instructions: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillDefinition {
    pub name: String,
    pub description: String,
    pub path: String,
    pub dir: String,
    pub content: String,
    pub metadata: SkillMetadata,
    pub source: SkillSource,
    pub plugin: Option<SkillPluginContext>,
    pub mermaid: Option<String>,
    pub d2: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub path: String,
    pub source: SkillSource,
    pub kind: Option<String>,
    pub disable_model_invocation: Option<bool>,
    pub is_sub_skill: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillRoot {
    pub path: String,
    pub source: SkillSource,
    pub plugin: Option<SkillPluginContext>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkippedSkill {
    pub path: String,
    pub kind: String,
    pub reason: String,
}

pub trait SkillCatalogContract: Send + Sync {
    fn get_skill(&self, name: &str) -> Option<SkillDefinition>;
    fn get_plugin_skill(&self, plugin_id: &str, name: &str) -> Option<SkillDefinition>;
    fn render_skill_prompt(
        &self,
        skill: &SkillDefinition,
        raw_args: &str,
        session_id: Option<&str>,
    ) -> String;
    fn render_skill_prompt_for_request(
        &self,
        skill: &SkillDefinition,
        raw_args: &str,
        session_id: Option<&str>,
    ) -> String;
    fn list_skills(&self) -> Vec<SkillDefinition>;
    fn list_invocable_skills(&self) -> Vec<SkillDefinition>;
    fn get_skill_roots(&self) -> Vec<String>;
    fn get_skipped_by_policy(&self) -> Vec<SkippedSkill>;
    fn get_model_skill_listing(&self) -> String;
}

// Original: normalizeSkillName().
pub fn normalize_skill_name(name: &str) -> String {
    name.to_lowercase()
}

// Original: isInlineSkillType().
pub fn is_inline_skill_type(kind: Option<&str>) -> bool {
    matches!(kind, None | Some("prompt" | "inline"))
}

// Original: isUserActivatableSkillType().
pub fn is_user_activatable_skill_type(kind: Option<&str>) -> bool {
    is_inline_skill_type(kind) || kind == Some("flow")
}

// Original: isSupportedSkillType().
pub fn is_supported_skill_type(kind: Option<&str>) -> bool {
    is_user_activatable_skill_type(kind) || kind == Some("reference")
}

// Original: summarizeSkill().
pub fn summarize_skill(skill: &SkillDefinition) -> SkillSummary {
    SkillSummary {
        name: skill.name.clone(),
        description: skill.description.clone(),
        path: skill.path.clone(),
        source: skill.source,
        kind: skill.metadata.kind.clone(),
        disable_model_invocation: skill.metadata.disable_model_invocation,
        is_sub_skill: skill.metadata.is_sub_skill,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_policies_match_prompt_inline_flow_and_reference_rules() {
        assert!(is_inline_skill_type(None));
        assert!(is_inline_skill_type(Some("prompt")));
        assert!(is_inline_skill_type(Some("inline")));
        assert!(!is_inline_skill_type(Some("flow")));
        assert!(is_user_activatable_skill_type(Some("flow")));
        assert!(is_supported_skill_type(Some("reference")));
        assert!(!is_supported_skill_type(Some("unknown")));
        assert_eq!(normalize_skill_name("Code-REVIEW"), "code-review");
    }

    #[test]
    fn summary_projects_only_the_original_public_fields() {
        let skill = SkillDefinition {
            name: "review".into(),
            description: "Review code".into(),
            path: "/skills/review/SKILL.md".into(),
            dir: "/skills/review".into(),
            content: "instructions".into(),
            metadata: SkillMetadata {
                kind: Some("flow".into()),
                disable_model_invocation: Some(true),
                is_sub_skill: Some(false),
                extra: Map::from_iter([("future".into(), Value::from(1))]),
                ..SkillMetadata::default()
            },
            source: SkillSource::Project,
            plugin: None,
            mermaid: Some("graph TD".into()),
            d2: None,
        };
        assert_eq!(
            summarize_skill(&skill),
            SkillSummary {
                name: "review".into(),
                description: "Review code".into(),
                path: "/skills/review/SKILL.md".into(),
                source: SkillSource::Project,
                kind: Some("flow".into()),
                disable_model_invocation: Some(true),
                is_sub_skill: Some(false),
            }
        );
    }
}
