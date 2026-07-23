//! Process-level skill discovery overrides.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/skillCatalogRuntimeOptions.ts`.

use std::sync::Arc;

use crate::_base::di::{
    descriptors::SyncDescriptor,
    instantiation::ServiceIdentifier,
    scope::{InstantiationType, LifecycleScope, register_scoped_service},
    service_collection::ServiceCollection,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SkillCatalogRuntimeOptions {
    pub explicit_dirs: Option<Vec<String>>,
}

impl SkillCatalogRuntimeOptions {
    // Original: SkillCatalogRuntimeOptions.constructor().
    pub fn new(explicit_dirs: Option<Vec<String>>) -> Self {
        Self { explicit_dirs }
    }
}

pub const SKILL_CATALOG_RUNTIME_OPTIONS_ID: ServiceIdentifier<SkillCatalogRuntimeOptions> =
    ServiceIdentifier::new("skillCatalogRuntimeOptions");

// Original: skillCatalogRuntimeOptionsSeed(). An absent or empty SDK/CLI list
// does not shadow the registered App-scope default.
pub fn skill_catalog_runtime_options_seed(explicit_dirs: Option<Vec<String>>) -> ServiceCollection {
    let mut seed = ServiceCollection::new();
    if explicit_dirs
        .as_ref()
        .is_some_and(|directories| !directories.is_empty())
    {
        seed.set_instance(
            SKILL_CATALOG_RUNTIME_OPTIONS_ID,
            Arc::new(SkillCatalogRuntimeOptions::new(explicit_dirs)),
        );
    }
    seed
}

pub fn register_skill_catalog_runtime_options() {
    register_scoped_service(
        LifecycleScope::App,
        SKILL_CATALOG_RUNTIME_OPTIONS_ID,
        SyncDescriptor::new(|_| Ok(SkillCatalogRuntimeOptions::default())),
        InstantiationType::Eager,
        "skillCatalog",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_only_contains_nonempty_explicit_directory_overrides() {
        assert!(
            skill_catalog_runtime_options_seed(None)
                .get(SKILL_CATALOG_RUNTIME_OPTIONS_ID)
                .unwrap()
                .is_none()
        );
        assert!(
            skill_catalog_runtime_options_seed(Some(Vec::new()))
                .get(SKILL_CATALOG_RUNTIME_OPTIONS_ID)
                .unwrap()
                .is_none()
        );

        let seed = skill_catalog_runtime_options_seed(Some(vec!["skills".into()]));
        assert_eq!(
            seed.get(SKILL_CATALOG_RUNTIME_OPTIONS_ID)
                .unwrap()
                .as_deref(),
            Some(&SkillCatalogRuntimeOptions {
                explicit_dirs: Some(vec!["skills".into()]),
            })
        );
        assert_eq!(
            SKILL_CATALOG_RUNTIME_OPTIONS_ID.to_string(),
            "skillCatalogRuntimeOptions"
        );
    }
}
