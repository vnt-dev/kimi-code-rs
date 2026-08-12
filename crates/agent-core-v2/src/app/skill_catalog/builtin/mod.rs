//! Code-defined built-in skills.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/builtin/*.ts`.

use std::sync::LazyLock;

use serde_json::Value;

use super::{
    parser::{ParseSkillTextOptions, parse_skill_text},
    registry::InMemorySkillCatalog,
    types::{SkillDefinition, SkillSource},
};

const CHECK_KIMI_CODE_DOCS_BODY: &str = include_str!("check-kimi-code-docs.md");
const CUSTOM_THEME_BODY: &str = include_str!("custom-theme.md");
const IMPORT_FROM_CC_CODEX_BODY: &str = include_str!("import-from-cc-codex.md");
const MCP_CONFIG_BODY: &str = include_str!("mcp-config.md");
const UPDATE_CONFIG_BODY: &str = include_str!("update-config.md");
const WRITE_GOAL_BODY: &str = include_str!("write-goal.md");
const SUB_SKILL_PARENT_BODY: &str = include_str!("sub-skill/SKILL.md");
const SUB_SKILL_REVIEW_BODY: &str = include_str!("sub-skill/review/SKILL.md");
const SUB_SKILL_CONSOLIDATE_BODY: &str = include_str!("sub-skill/consolidate/SKILL.md");

pub static CHECK_KIMI_CODE_DOCS_SKILL: LazyLock<SkillDefinition> = LazyLock::new(|| {
    make_flat_builtin(
        CHECK_KIMI_CODE_DOCS_BODY,
        "check-kimi-code-docs",
        "builtin://check-kimi-code-docs",
        false,
    )
});

pub static CUSTOM_THEME_SKILL: LazyLock<SkillDefinition> = LazyLock::new(|| {
    make_flat_builtin(
        CUSTOM_THEME_BODY,
        "custom-theme",
        "builtin://custom-theme",
        true,
    )
});

pub static IMPORT_FROM_CC_CODEX_SKILL: LazyLock<SkillDefinition> = LazyLock::new(|| {
    make_flat_builtin(
        IMPORT_FROM_CC_CODEX_BODY,
        "import-from-cc-codex",
        "builtin://import-from-cc-codex",
        true,
    )
});

pub static MCP_CONFIG_SKILL: LazyLock<SkillDefinition> = LazyLock::new(|| {
    make_flat_builtin(MCP_CONFIG_BODY, "mcp-config", "builtin://mcp-config", true)
});

pub static UPDATE_CONFIG_SKILL: LazyLock<SkillDefinition> = LazyLock::new(|| {
    make_flat_builtin(
        UPDATE_CONFIG_BODY,
        "update-config",
        "builtin://update-config",
        false,
    )
});

pub static WRITE_GOAL_SKILL: LazyLock<SkillDefinition> = LazyLock::new(|| {
    make_flat_builtin(WRITE_GOAL_BODY, "write-goal", "builtin://write-goal", false)
});

pub static SUB_SKILL_PARENT: LazyLock<SkillDefinition> = LazyLock::new(|| {
    let mut skill = make_bundle_builtin(SUB_SKILL_PARENT_BODY, "sub-skill", "builtin://sub-skill");
    skill.metadata.disable_model_invocation = Some(true);
    skill
        .metadata
        .extra
        .insert("has-sub-skill".into(), Value::Bool(true));
    skill
});

pub static SUB_SKILL_REVIEW: LazyLock<SkillDefinition> = LazyLock::new(|| {
    let mut skill = make_bundle_builtin(
        SUB_SKILL_REVIEW_BODY,
        "sub-skill.review",
        "builtin://sub-skill/review",
    );
    skill.metadata.disable_model_invocation = Some(true);
    skill.metadata.is_sub_skill = Some(true);
    skill
});

pub static SUB_SKILL_CONSOLIDATE: LazyLock<SkillDefinition> = LazyLock::new(|| {
    let mut skill = make_bundle_builtin(
        SUB_SKILL_CONSOLIDATE_BODY,
        "sub-skill.consolidate",
        "builtin://sub-skill/consolidate",
    );
    skill.metadata.disable_model_invocation = Some(true);
    skill.metadata.is_sub_skill = Some(true);
    skill
});

pub static BUILTIN_SKILLS: LazyLock<Vec<SkillDefinition>> = LazyLock::new(|| {
    vec![
        MCP_CONFIG_SKILL.clone(),
        IMPORT_FROM_CC_CODEX_SKILL.clone(),
        UPDATE_CONFIG_SKILL.clone(),
        CUSTOM_THEME_SKILL.clone(),
        WRITE_GOAL_SKILL.clone(),
        CHECK_KIMI_CODE_DOCS_SKILL.clone(),
        SUB_SKILL_PARENT.clone(),
        SUB_SKILL_REVIEW.clone(),
        SUB_SKILL_CONSOLIDATE.clone(),
    ]
});

// Original: registerBuiltinSkills().
pub fn register_builtin_skills(registry: &mut InMemorySkillCatalog) {
    for skill in BUILTIN_SKILLS.iter() {
        registry.register_builtin_skill(skill.clone());
    }
}

fn make_flat_builtin(
    body: &str,
    name: &str,
    pseudo_path: &str,
    disable_model_invocation: bool,
) -> SkillDefinition {
    let source_path = format!("/builtin/skills/{name}.md");
    let mut skill = parse_builtin(body, &source_path, name);
    skill.path = pseudo_path.into();
    skill.dir = pseudo_path.into();
    skill.metadata.kind.get_or_insert_with(|| "inline".into());
    if disable_model_invocation {
        skill.metadata.disable_model_invocation = Some(true);
    }
    skill
}

fn make_bundle_builtin(body: &str, name: &str, pseudo_path: &str) -> SkillDefinition {
    let source_path = format!("/builtin/skills/{name}/SKILL.md");
    let mut skill = parse_builtin(body, &source_path, name);
    skill.name = name.into();
    skill.path = pseudo_path.into();
    skill.dir = pseudo_path.into();
    skill.metadata.kind.get_or_insert_with(|| "inline".into());
    skill
}

fn parse_builtin(body: &str, source_path: &str, name: &str) -> SkillDefinition {
    match parse_skill_text(ParseSkillTextOptions {
        skill_md_path: source_path,
        skill_dir_name: name,
        source: SkillSource::Builtin,
        text: body,
    }) {
        Ok(skill) => skill,
        Err(error) => panic!("invalid built-in skill {name}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::_base::utils::hash::encode_hex;

    #[test]
    fn builtins_preserve_order_paths_and_invocation_metadata() {
        assert_eq!(
            BUILTIN_SKILLS
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            [
                "mcp-config",
                "import-from-cc-codex",
                "update-config",
                "custom-theme",
                "write-goal",
                "check-kimi-code-docs",
                "sub-skill",
                "sub-skill.review",
                "sub-skill.consolidate",
            ]
        );
        assert!(
            BUILTIN_SKILLS
                .iter()
                .all(|skill| skill.source == SkillSource::Builtin
                    && skill.path.starts_with("builtin://")
                    && skill.path == skill.dir
                    && skill.metadata.kind.as_deref() == Some("inline"))
        );
        assert_eq!(
            SUB_SKILL_PARENT.metadata.disable_model_invocation,
            Some(true)
        );
        assert_eq!(SUB_SKILL_PARENT.metadata.extra["has-sub-skill"], true);
        assert_eq!(SUB_SKILL_REVIEW.metadata.is_sub_skill, Some(true));
        assert_eq!(SUB_SKILL_CONSOLIDATE.metadata.is_sub_skill, Some(true));
    }

    #[test]
    fn embedded_markdown_matches_the_original_assets_byte_for_byte() {
        let mut hasher = Sha256::new();
        for body in [
            MCP_CONFIG_BODY,
            IMPORT_FROM_CC_CODEX_BODY,
            UPDATE_CONFIG_BODY,
            CUSTOM_THEME_BODY,
            WRITE_GOAL_BODY,
            CHECK_KIMI_CODE_DOCS_BODY,
            SUB_SKILL_PARENT_BODY,
            SUB_SKILL_REVIEW_BODY,
            SUB_SKILL_CONSOLIDATE_BODY,
        ] {
            hasher.update(body.as_bytes());
        }
        let digest = encode_hex(hasher.finalize());
        assert_eq!(
            digest,
            "b231b7a53025b5690cd5f5f9a77e253650c13f2197f138e36d19526e0ec685c0"
        );
    }

    #[test]
    fn registers_every_builtin_in_the_in_memory_catalog() {
        let mut catalog = InMemorySkillCatalog::default();
        register_builtin_skills(&mut catalog);
        assert_eq!(catalog.list_skills().len(), BUILTIN_SKILLS.len());
        assert!(catalog.get_skill("MCP-CONFIG").is_some());
        assert!(catalog.get_skill("sub-skill.consolidate").is_some());
    }
}
