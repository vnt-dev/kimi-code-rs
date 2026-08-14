//! In-memory skill discovery backend.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/inMemorySkillDiscovery.ts`.

use parking_lot::RwLock;
use std::sync::Arc;

use async_trait::async_trait;

use crate::_base::di::{
    descriptors::SyncDescriptor,
    scope::{InstantiationType, LifecycleScope, register_scoped_service},
};

use super::{
    discovery::{
        SKILL_DISCOVERY_SERVICE_ID, SkillDiscoveryContract, SkillDiscoveryHandle,
        SkillDiscoveryResult,
    },
    types::{SkillDefinition, SkillRoot, SkillSource},
};

#[derive(Default)]
struct PresetSkills {
    project: Vec<SkillDefinition>,
    user: Vec<SkillDefinition>,
    plugin: Vec<SkillDefinition>,
    extra: Vec<SkillDefinition>,
}

#[derive(Default)]
pub struct InMemorySkillDiscovery {
    presets: RwLock<PresetSkills>,
}

impl InMemorySkillDiscovery {
    // Original: InMemorySkillDiscovery.setProjectSkills().
    pub fn set_project_skills(&self, skills: &[SkillDefinition]) {
        self.presets_write().project = skills.to_vec();
    }

    // Original: InMemorySkillDiscovery.setUserSkills().
    pub fn set_user_skills(&self, skills: &[SkillDefinition]) {
        self.presets_write().user = skills.to_vec();
    }

    // Original: InMemorySkillDiscovery.setPluginSkills().
    pub fn set_plugin_skills(&self, skills: &[SkillDefinition]) {
        self.presets_write().plugin = skills.to_vec();
    }

    // Original: InMemorySkillDiscovery.setExtraSkills().
    pub fn set_extra_skills(&self, skills: &[SkillDefinition]) {
        self.presets_write().extra = skills.to_vec();
    }

    fn presets_write(&self) -> parking_lot::RwLockWriteGuard<'_, PresetSkills> {
        self.presets.write()
    }
}

#[async_trait]
impl SkillDiscoveryContract for InMemorySkillDiscovery {
    // Original: InMemorySkillDiscovery.discover().
    async fn discover(&self, roots: &[SkillRoot]) -> SkillDiscoveryResult {
        let presets = self.presets.read();
        let mut skills = Vec::new();
        if roots.is_empty() {
            skills.extend(presets.user.clone());
            skills.extend(presets.project.clone());
        } else {
            if roots.iter().any(|root| root.plugin.is_some()) {
                skills.extend(presets.plugin.clone());
            }
            if roots.iter().any(|root| root.source == SkillSource::Extra) {
                skills.extend(presets.extra.clone());
            }
            if roots.iter().any(|root| root.source == SkillSource::User) {
                skills.extend(presets.user.clone());
            }
            if roots.iter().any(|root| root.source == SkillSource::Project) {
                skills.extend(presets.project.clone());
            }
        }
        SkillDiscoveryResult {
            skills,
            skipped: Vec::new(),
            scanned_roots: Vec::new(),
        }
    }
}

pub fn register_in_memory_skill_discovery() {
    register_scoped_service(
        LifecycleScope::App,
        SKILL_DISCOVERY_SERVICE_ID,
        SyncDescriptor::new(|_| {
            let service: Arc<dyn SkillDiscoveryContract> =
                Arc::new(InMemorySkillDiscovery::default());
            Ok(SkillDiscoveryHandle(service))
        }),
        InstantiationType::Eager,
        "skillCatalog",
    );
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;
    use crate::app::skill_catalog::types::{SkillMetadata, SkillPluginContext};

    fn skill(name: &str, source: SkillSource) -> SkillDefinition {
        SkillDefinition {
            name: name.into(),
            description: name.into(),
            path: format!("/{name}/SKILL.md"),
            dir: format!("/{name}"),
            content: name.into(),
            metadata: SkillMetadata {
                extra: Map::new(),
                ..SkillMetadata::default()
            },
            source,
            plugin: None,
            mermaid: None,
            d2: None,
        }
    }

    #[tokio::test]
    async fn empty_roots_return_user_then_project_presets() {
        let discovery = InMemorySkillDiscovery::default();
        discovery.set_project_skills(&[skill("project", SkillSource::Project)]);
        discovery.set_user_skills(&[skill("user", SkillSource::User)]);
        assert_eq!(
            discovery
                .discover(&[])
                .await
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            ["user", "project"]
        );
    }

    #[tokio::test]
    async fn roots_select_presets_in_plugin_extra_user_project_order() {
        let discovery = InMemorySkillDiscovery::default();
        discovery.set_plugin_skills(&[skill("plugin", SkillSource::Builtin)]);
        discovery.set_extra_skills(&[skill("extra", SkillSource::Extra)]);
        discovery.set_user_skills(&[skill("user", SkillSource::User)]);
        discovery.set_project_skills(&[skill("project", SkillSource::Project)]);
        let roots = vec![
            SkillRoot {
                path: "/project".into(),
                source: SkillSource::Project,
                plugin: None,
            },
            SkillRoot {
                path: "/plugin".into(),
                source: SkillSource::Builtin,
                plugin: Some(SkillPluginContext {
                    id: "plugin".into(),
                    instructions: None,
                }),
            },
            SkillRoot {
                path: "/extra".into(),
                source: SkillSource::Extra,
                plugin: None,
            },
        ];
        let result = discovery.discover(&roots).await;
        assert_eq!(
            result
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            ["plugin", "extra", "project"]
        );
        assert!(result.skipped.is_empty());
        assert!(result.scanned_roots.is_empty());
    }
}
