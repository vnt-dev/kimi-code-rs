//! Session-scoped skill sources and merged catalog.

pub mod contract;

pub use contract::{
    SESSION_SKILL_CATALOG_ID, SessionSkillCatalogContract, SessionSkillCatalogHandle,
    SkillCatalogSinkContract, SkillCatalogSinkOptions,
};
