//! Skill discovery, parsing, and catalog services.

pub mod parser;
pub mod types;

pub use parser::{
    FrontmatterError, ParseSkillError, ParseSkillTextOptions, ParsedFrontmatter, SkillParseError,
    UnsupportedSkillTypeError, parse_d2_flowchart, parse_frontmatter, parse_mermaid_flowchart,
    parse_skill_text, skill_argument_names,
};
pub use types::{
    SkillDefinition, SkillMetadata, SkillPluginContext, SkillRoot, SkillSource, SkillSummary,
    SkippedSkill, is_inline_skill_type, is_supported_skill_type, is_user_activatable_skill_type,
    normalize_skill_name, summarize_skill,
};
