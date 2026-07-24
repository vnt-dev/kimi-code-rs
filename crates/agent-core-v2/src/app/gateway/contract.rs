use crate::_base::di::instantiation::ServiceIdentifier;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use std::{error::Error, ops::Deref, sync::Arc};
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct TurnIdResponse {
    pub turn_id: u64,
}
pub type GatewayResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
#[async_trait]
pub trait RestGatewayContract: Send + Sync {
    async fn prompt(
        &self,
        session_id: &str,
        agent_id: &str,
        input: &str,
    ) -> GatewayResult<Option<TurnIdResponse>>;
    async fn steer(
        &self,
        session_id: &str,
        agent_id: &str,
        content: &str,
    ) -> GatewayResult<Option<TurnIdResponse>>;
    async fn cancel(
        &self,
        session_id: &str,
        agent_id: &str,
        reason: Option<&str>,
    ) -> GatewayResult<()>;
    async fn get_status(&self, session_id: &str) -> GatewayResult<Value>;
    async fn flush_logs(&self, session_id: &str) -> GatewayResult<()>;
    async fn flush_global_logs(&self) -> GatewayResult<()>;
}
#[derive(Clone)]
pub struct RestGatewayHandle(pub Arc<dyn RestGatewayContract>);
impl Deref for RestGatewayHandle {
    type Target = dyn RestGatewayContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
pub const REST_GATEWAY_SERVICE_ID: ServiceIdentifier<RestGatewayHandle> =
    ServiceIdentifier::new("restGateway");
pub trait WsGatewayContract: Send + Sync {
    fn connect(&self, connection_id: &str);
    fn broadcast(&self, session_id: &str, event: Value);
}
#[derive(Clone)]
pub struct WsGatewayHandle(pub Arc<dyn WsGatewayContract>);
impl Deref for WsGatewayHandle {
    type Target = dyn WsGatewayContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
pub const WS_GATEWAY_SERVICE_ID: ServiceIdentifier<WsGatewayHandle> =
    ServiceIdentifier::new("wsGateway");
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gateway_identities_and_turn_wire_name_match_source() {
        assert_eq!(REST_GATEWAY_SERVICE_ID.to_string(), "restGateway");
        assert_eq!(WS_GATEWAY_SERVICE_ID.to_string(), "wsGateway");
        assert_eq!(
            serde_json::to_value(TurnIdResponse { turn_id: 3 }).unwrap(),
            serde_json::json!({"turn_id":3})
        );
    }
}
