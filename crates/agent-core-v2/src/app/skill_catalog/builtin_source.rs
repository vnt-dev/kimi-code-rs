//! Built-in skill contribution source.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/builtinSkillSource.ts`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::_base::di::{
    descriptors::SyncDescriptor,
    instantiation::ServiceIdentifier,
    scope::{InstantiationType, LifecycleScope, register_scoped_service},
};

use super::{
    builtin::BUILTIN_SKILLS,
    source::{SKILL_SOURCE_PRIORITY, SkillContribution, SkillSourceContract},
};

#[derive(Default)]
pub struct BuiltinSkillSource;

#[async_trait]
impl SkillSourceContract for BuiltinSkillSource {
    fn id(&self) -> &str {
        "builtin"
    }

    fn priority(&self) -> i32 {
        SKILL_SOURCE_PRIORITY.builtin
    }

    // Original: BuiltinSkillSource.load().
    async fn load(&self) -> SkillContribution {
        SkillContribution {
            skills: BUILTIN_SKILLS.clone(),
            skipped: None,
            scanned_roots: None,
        }
    }
}

#[derive(Clone)]
pub struct BuiltinSkillSourceHandle(pub Arc<dyn SkillSourceContract>);

impl Deref for BuiltinSkillSourceHandle {
    type Target = dyn SkillSourceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const BUILTIN_SKILL_SOURCE_ID: ServiceIdentifier<BuiltinSkillSourceHandle> =
    ServiceIdentifier::new("builtinSkillSource");

pub fn register_builtin_skill_source() {
    register_scoped_service(
        LifecycleScope::App,
        BUILTIN_SKILL_SOURCE_ID,
        SyncDescriptor::new(|_| {
            let source: Arc<dyn SkillSourceContract> = Arc::new(BuiltinSkillSource);
            Ok(BuiltinSkillSourceHandle(source))
        }),
        InstantiationType::Eager,
        "skillCatalog",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn loads_all_builtins_at_the_original_priority() {
        let source = BuiltinSkillSource;
        assert_eq!(source.id(), "builtin");
        assert_eq!(source.priority(), 0);
        let contribution = source.load().await;
        assert_eq!(contribution.skills.as_slice(), BUILTIN_SKILLS.as_slice());
        assert!(contribution.skipped.is_none());
        assert!(contribution.scanned_roots.is_none());
        assert_eq!(BUILTIN_SKILL_SOURCE_ID.to_string(), "builtinSkillSource");
    }
}
