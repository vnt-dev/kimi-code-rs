//! Skill discovery, parsing, and catalog services.

pub mod parser;
pub mod types;

pub use parser::{FrontmatterError, ParsedFrontmatter, parse_frontmatter};
pub use types::{
    SkillDefinition, SkillMetadata, SkillPluginContext, SkillRoot, SkillSource, SkillSummary,
    SkippedSkill, is_inline_skill_type, is_supported_skill_type, is_user_activatable_skill_type,
    normalize_skill_name, summarize_skill,
};
