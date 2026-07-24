//! Session-scoped skill sources and merged catalog.

pub mod contract;
pub mod explicit_source;
pub mod extra_source;
pub mod plugin_source;
pub mod workspace_source;

pub use contract::{
    SESSION_SKILL_CATALOG_ID, SessionSkillCatalogContract, SessionSkillCatalogHandle,
    SkillCatalogSinkContract, SkillCatalogSinkOptions,
};
pub use explicit_source::{
    EXPLICIT_FILE_SKILL_SOURCE_ID, ExplicitFileSkillSource, ExplicitFileSkillSourceHandle,
    register_explicit_file_skill_source,
};
pub use extra_source::{
    EXTRA_FILE_SKILL_SOURCE_ID, ExtraFileSkillSource, ExtraFileSkillSourceHandle,
    register_extra_file_skill_source,
};
pub use plugin_source::{
    PLUGIN_SKILL_SOURCE_ID, PluginSkillSource, PluginSkillSourceHandle,
    register_plugin_skill_source,
};
pub use workspace_source::{
    WORKSPACE_FILE_SKILL_SOURCE_ID, WorkspaceFileSkillSource, WorkspaceFileSkillSourceHandle,
    register_workspace_file_skill_source,
};
