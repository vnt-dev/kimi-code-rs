//! Session-scoped skill sources and merged catalog.

pub mod contract;
pub mod explicit_source;

pub use contract::{
    SESSION_SKILL_CATALOG_ID, SessionSkillCatalogContract, SessionSkillCatalogHandle,
    SkillCatalogSinkContract, SkillCatalogSinkOptions,
};
pub use explicit_source::{
    EXPLICIT_FILE_SKILL_SOURCE_ID, ExplicitFileSkillSource, ExplicitFileSkillSourceHandle,
    register_explicit_file_skill_source,
};
