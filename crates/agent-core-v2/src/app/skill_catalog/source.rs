//! Skill contribution source contract.
//!
//! Original: `packages/agent-core-v2/src/app/skillCatalog/skillSource.ts`.

use async_trait::async_trait;

use crate::_base::event::Event;
use crate::app::config::ConfigServiceError;

use super::types::{SkillDefinition, SkippedSkill};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SkillContribution {
    pub skills: Vec<SkillDefinition>,
    pub skipped: Option<Vec<SkippedSkill>>,
    pub scanned_roots: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkillSourcePriorities {
    pub builtin: i32,
    pub plugin: i32,
    pub extra: i32,
    pub user: i32,
    pub workspace: i32,
}

pub const SKILL_SOURCE_PRIORITY: SkillSourcePriorities = SkillSourcePriorities {
    builtin: 0,
    plugin: 5,
    extra: 10,
    user: 20,
    workspace: 30,
};

#[derive(Debug, thiserror::Error)]
pub enum SkillSourceError {
    #[error(transparent)]
    Config(#[from] ConfigServiceError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("plugin skill source failed: {0}")]
    Plugin(Box<dyn std::error::Error + Send + Sync>),

    #[error("initial skill catalog load failed: {0}")]
    Cached(Box<dyn std::error::Error + Send + Sync>),
}

pub type SkillSourceResult<T> = Result<T, SkillSourceError>;

#[async_trait]
pub trait SkillSourceContract: Send + Sync {
    fn id(&self) -> &str;
    fn priority(&self) -> i32;

    fn on_did_change(&self) -> Option<Event<()>> {
        None
    }

    async fn load(&self) -> SkillSourceResult<SkillContribution>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_priorities_preserve_override_order() {
        let priorities = SKILL_SOURCE_PRIORITY;
        assert_eq!(
            priorities,
            SkillSourcePriorities {
                builtin: 0,
                plugin: 5,
                extra: 10,
                user: 20,
                workspace: 30,
            }
        );
    }
}
