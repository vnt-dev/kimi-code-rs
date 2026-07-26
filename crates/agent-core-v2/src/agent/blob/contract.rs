use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    _base::di::instantiation::ServiceIdentifier, kosong::contract::message::ContentPart,
    persistence::interface::storage::StorageError,
};

pub const BLOBREF_PROTOCOL: &str = "blobref:";
pub const MISSING_MEDIA_PLACEHOLDER: &str = "[media missing]";

#[async_trait]
pub trait AgentBlobServiceContract: Send + Sync {
    async fn offload_parts(
        &self,
        parts: Vec<ContentPart>,
    ) -> Result<Vec<ContentPart>, StorageError>;
    async fn load_parts(&self, parts: Vec<ContentPart>) -> Vec<ContentPart>;
    fn is_blob_ref(&self, url: &str) -> bool;

    /// Wire-level structural transformation. Unlike the typed public methods,
    /// this preserves legacy and extension fields on content-part objects.
    async fn offload_wire_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String>;
    async fn load_wire_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String>;
}

#[derive(Clone)]
pub struct AgentBlobServiceHandle(pub Arc<dyn AgentBlobServiceContract>);

impl Deref for AgentBlobServiceHandle {
    type Target = dyn AgentBlobServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

// Original: agentBlobService.ts, IAgentBlobService.
pub const AGENT_BLOB_SERVICE_ID: ServiceIdentifier<AgentBlobServiceHandle> =
    ServiceIdentifier::new("agentBlobService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_and_identifier_match_source() {
        assert_eq!(BLOBREF_PROTOCOL, "blobref:");
        assert_eq!(MISSING_MEDIA_PLACEHOLDER, "[media missing]");
        assert_eq!(AGENT_BLOB_SERVICE_ID.to_string(), "agentBlobService");
    }
}
