//! Flag-definition registry contract and decentralized contributions.
//!
//! Original: `packages/agent-core-v2/src/app/flag/flagRegistry.ts`.

use std::{
    ops::Deref,
    sync::{Arc, LazyLock, RwLock},
};

use serde::{Deserialize, Serialize};

use crate::_base::di::{instantiation::ServiceIdentifier, lifecycle::DisposableHandle};

pub type FlagId = String;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FlagSurface {
    Core,
    Tui,
    Both,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlagDefinitionInput {
    pub id: FlagId,
    pub title: String,
    pub description: String,
    pub env: String,
    pub default: bool,
    pub surface: FlagSurface,
}

static CONTRIBUTED_FLAGS: LazyLock<RwLock<Vec<FlagDefinitionInput>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

// Original: registerFlagDefinition(). As in TypeScript, duplicate contributions
// are accepted here and rejected when a registry drains the contribution list.
pub fn register_flag_definition(definition: FlagDefinitionInput) {
    CONTRIBUTED_FLAGS.write().unwrap().push(definition);
}

// Original: getContributedFlags(). The clone is the Rust ownership adaptation
// of returning the process-wide array as a readonly view.
pub fn get_contributed_flags() -> Vec<FlagDefinitionInput> {
    CONTRIBUTED_FLAGS.read().unwrap().clone()
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum FlagRegistryError {
    #[error("Flag '{0}' is already registered")]
    AlreadyRegistered(FlagId),
}

pub trait FlagRegistry: Send + Sync {
    fn register(
        &self,
        definition: FlagDefinitionInput,
    ) -> Result<DisposableHandle, FlagRegistryError>;
    fn get(&self, id: &str) -> Option<FlagDefinitionInput>;
    fn list(&self) -> Vec<FlagDefinitionInput>;
}

#[derive(Clone)]
pub struct FlagRegistryHandle(pub Arc<dyn FlagRegistry>);

impl Deref for FlagRegistryHandle {
    type Target = dyn FlagRegistry;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const FLAG_REGISTRY_SERVICE_ID: ServiceIdentifier<FlagRegistryHandle> =
    ServiceIdentifier::new("flagRegistry");
