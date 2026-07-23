//! Backend-neutral skill discovery contract.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/skillDiscovery.ts`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::_base::di::instantiation::ServiceIdentifier;

use super::types::{SkillDefinition, SkillRoot, SkippedSkill};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SkillDiscoveryResult {
    pub skills: Vec<SkillDefinition>,
    pub skipped: Vec<SkippedSkill>,
    pub scanned_roots: Vec<String>,
}

#[async_trait]
pub trait SkillDiscoveryContract: Send + Sync {
    async fn discover(&self, roots: &[SkillRoot]) -> SkillDiscoveryResult;
}

#[derive(Clone)]
pub struct SkillDiscoveryHandle(pub Arc<dyn SkillDiscoveryContract>);

impl Deref for SkillDiscoveryHandle {
    type Target = dyn SkillDiscoveryContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SKILL_DISCOVERY_SERVICE_ID: ServiceIdentifier<SkillDiscoveryHandle> =
    ServiceIdentifier::new("skillDiscovery");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identity_matches_source() {
        assert_eq!(SKILL_DISCOVERY_SERVICE_ID.to_string(), "skillDiscovery");
    }
}
