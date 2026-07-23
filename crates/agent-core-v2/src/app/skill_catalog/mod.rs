//! Skill discovery, parsing, and catalog services.

pub mod discovery;
pub mod in_memory_discovery;
pub mod parser;
pub mod types;

pub use discovery::{
    SKILL_DISCOVERY_SERVICE_ID, SkillDiscoveryContract, SkillDiscoveryHandle, SkillDiscoveryResult,
};
pub use in_memory_discovery::{InMemorySkillDiscovery, register_in_memory_skill_discovery};
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
