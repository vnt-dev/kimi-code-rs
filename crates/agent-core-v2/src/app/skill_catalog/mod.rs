//! Skill discovery, parsing, and catalog services.

pub mod config_section;
pub mod discovery;
pub mod errors;
pub mod file_discovery;
pub mod in_memory_discovery;
pub mod parser;
pub mod roots;
pub mod runtime_options;
pub mod source;
pub mod types;

pub use config_section::{
    EXTRA_SKILL_DIRS_CONFIG_SCHEMA, EXTRA_SKILL_DIRS_SECTION,
    MERGE_ALL_AVAILABLE_SKILLS_CONFIG_SCHEMA, MERGE_ALL_AVAILABLE_SKILLS_SECTION,
    register_skill_catalog_config_sections,
};
pub use discovery::{
    SKILL_DISCOVERY_SERVICE_ID, SkillDiscoveryContract, SkillDiscoveryHandle, SkillDiscoveryResult,
};
pub use errors::{
    SKILL_ERRORS, SKILL_NAME_EMPTY, SKILL_NOT_FOUND, SKILL_TYPE_UNSUPPORTED,
    ensure_skill_errors_registered,
};
pub use file_discovery::{FileSkillDiscovery, discover_file_skills, register_file_skill_discovery};
pub use in_memory_discovery::{InMemorySkillDiscovery, register_in_memory_skill_discovery};
pub use parser::{
    FrontmatterError, ParseSkillError, ParseSkillTextOptions, ParsedFrontmatter, SkillParseError,
    UnsupportedSkillTypeError, parse_d2_flowchart, parse_frontmatter, parse_mermaid_flowchart,
    parse_skill_text, skill_argument_names,
};
pub use roots::{SkillRootsOptions, configured_roots, project_roots, user_roots};
pub use runtime_options::{
    SKILL_CATALOG_RUNTIME_OPTIONS_ID, SkillCatalogRuntimeOptions,
    register_skill_catalog_runtime_options, skill_catalog_runtime_options_seed,
};
pub use source::{
    SKILL_SOURCE_PRIORITY, SkillContribution, SkillSourceContract, SkillSourcePriorities,
};
pub use types::{
    SkillDefinition, SkillMetadata, SkillPluginContext, SkillRoot, SkillSource, SkillSummary,
    SkippedSkill, is_inline_skill_type, is_supported_skill_type, is_user_activatable_skill_type,
    normalize_skill_name, summarize_skill,
};
