//! Session-scoped merged skill catalog and contribution sink contracts.
//!
//! Original: `packages/agent-core-v2/src/session/sessionSkillCatalog/skillCatalog.ts`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::{
        di::{
            instantiation::ServiceIdentifier,
            lifecycle::{Disposable, DisposeResult},
        },
        event::Event,
    },
    app::skill_catalog::{SkillCatalogContract, SkillContribution, SkillSourceResult},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkillCatalogSinkOptions {
    pub priority: i32,
}

pub trait SkillCatalogSinkContract: Send + Sync {
    fn set(&self, id: &str, contribution: SkillContribution, options: SkillCatalogSinkOptions);
    fn remove(&self, id: &str);
}

#[async_trait]
pub trait SessionSkillCatalogContract: SkillCatalogSinkContract + Disposable + Send + Sync {
    fn catalog(&self) -> Arc<dyn SkillCatalogContract>;
    fn on_did_change(&self) -> Event<String>;
    async fn ready(&self) -> SkillSourceResult<()>;
    async fn load(&self) -> SkillSourceResult<()>;
    async fn reload(&self) -> SkillSourceResult<()>;
}

#[derive(Clone)]
pub struct SessionSkillCatalogHandle(pub Arc<dyn SessionSkillCatalogContract>);

impl Deref for SessionSkillCatalogHandle {
    type Target = dyn SessionSkillCatalogContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for SessionSkillCatalogHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const SESSION_SKILL_CATALOG_ID: ServiceIdentifier<SessionSkillCatalogHandle> =
    ServiceIdentifier::new("sessionSkillCatalog");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identity_matches_the_original_decorator() {
        assert_eq!(SESSION_SKILL_CATALOG_ID.to_string(), "sessionSkillCatalog");
        assert_eq!(SkillCatalogSinkOptions { priority: 10 }.priority, 10);
    }
}
