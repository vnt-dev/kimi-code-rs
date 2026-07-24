use crate::{
    _base::di::instantiation::ServiceIdentifier,
    agent::context_memory::protocol_message::{MessageRole, ProtocolMessage},
};
use async_trait::async_trait;
use std::{error::Error, ops::Deref, sync::Arc};
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CursorQuery {
    pub before_id: Option<String>,
    pub after_id: Option<String>,
    pub page_size: Option<usize>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MessageListQuery {
    pub before_id: Option<String>,
    pub after_id: Option<String>,
    pub page_size: Option<usize>,
    pub role: Option<MessageRole>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct PageResponse<T> {
    pub items: Vec<T>,
    pub has_more: bool,
}
pub type MessageLegacyResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
#[async_trait]
pub trait MessageLegacyServiceContract: Send + Sync {
    async fn list(
        &self,
        session_id: &str,
        query: MessageListQuery,
    ) -> MessageLegacyResult<PageResponse<ProtocolMessage>>;
    async fn get(&self, session_id: &str, message_id: &str)
    -> MessageLegacyResult<ProtocolMessage>;
}
#[derive(Clone)]
pub struct MessageLegacyServiceHandle(pub Arc<dyn MessageLegacyServiceContract>);
impl Deref for MessageLegacyServiceHandle {
    type Target = dyn MessageLegacyServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
pub const MESSAGE_LEGACY_SERVICE_ID: ServiceIdentifier<MessageLegacyServiceHandle> =
    ServiceIdentifier::new("messageLegacyService");
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_identity_match_source() {
        assert_eq!(
            MessageListQuery::default(),
            MessageListQuery {
                before_id: None,
                after_id: None,
                page_size: None,
                role: None
            }
        );
        assert_eq!(
            MESSAGE_LEGACY_SERVICE_ID.to_string(),
            "messageLegacyService"
        );
    }
}
