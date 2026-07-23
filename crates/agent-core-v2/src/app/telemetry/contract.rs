//! Telemetry service and appender contracts.
//!
//! Original: `packages/agent-core-v2/src/app/telemetry/telemetry.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use indexmap::IndexMap;
use serde_json::Value;

use crate::_base::di::{
    instantiation::ServiceIdentifier,
    lifecycle::{DisposableHandle, disposable_none},
};

// `None` represents JavaScript `undefined`; `Some(Value::Null)` remains an
// explicit null. Callers must provide primitive JSON values.
pub type TelemetryProperties = IndexMap<String, Option<Value>>;
pub type TelemetryContextPatch = TelemetryProperties;
pub type TelemetryAppenderError = Box<dyn Error + Send + Sync>;
pub type TelemetryAppenderResult = Result<(), TelemetryAppenderError>;

#[async_trait]
pub trait TelemetryAppender: Send + Sync {
    fn track(&self, event: &str, properties: Option<&TelemetryProperties>);

    fn with_context(&self, _patch: &TelemetryContextPatch) -> Option<Arc<dyn TelemetryAppender>> {
        None
    }

    fn set_context(&self, _patch: &TelemetryContextPatch) {}

    async fn flush(&self) -> TelemetryAppenderResult {
        Ok(())
    }

    async fn shutdown(&self) -> TelemetryAppenderResult {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct TelemetryServiceOptions {
    pub appender: Option<Arc<dyn TelemetryAppender>>,
    pub appenders: Vec<Arc<dyn TelemetryAppender>>,
    pub context: TelemetryProperties,
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub turn_id: Option<String>,
}

#[async_trait]
pub trait TelemetryServiceContract: Send + Sync {
    fn track(&self, event: &str, properties: Option<&TelemetryProperties>);
    fn with_context(&self, patch: &TelemetryContextPatch) -> TelemetryServiceHandle;
    fn set_context(&self, patch: &TelemetryContextPatch);
    fn add_appender(&self, appender: Arc<dyn TelemetryAppender>) -> DisposableHandle;
    fn remove_appender(&self, appender: &Arc<dyn TelemetryAppender>);
    fn set_appender(&self, appender: Arc<dyn TelemetryAppender>);
    fn set_enabled(&self, enabled: bool);
    async fn flush(&self);
    async fn shutdown(&self);
}

#[derive(Clone)]
pub struct TelemetryServiceHandle(pub Arc<dyn TelemetryServiceContract>);

impl Deref for TelemetryServiceHandle {
    type Target = dyn TelemetryServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const TELEMETRY_SERVICE_ID: ServiceIdentifier<TelemetryServiceHandle> =
    ServiceIdentifier::new("agentTelemetryService");

#[derive(Debug, Default)]
pub struct NullTelemetryAppender;

#[async_trait]
impl TelemetryAppender for NullTelemetryAppender {
    fn track(&self, _event: &str, _properties: Option<&TelemetryProperties>) {}

    fn with_context(&self, _patch: &TelemetryContextPatch) -> Option<Arc<dyn TelemetryAppender>> {
        Some(Arc::new(Self))
    }
}

#[derive(Debug, Default)]
pub struct NoopTelemetryService;

#[async_trait]
impl TelemetryServiceContract for NoopTelemetryService {
    fn track(&self, _event: &str, _properties: Option<&TelemetryProperties>) {}

    fn with_context(&self, _patch: &TelemetryContextPatch) -> TelemetryServiceHandle {
        TelemetryServiceHandle(Arc::new(Self))
    }

    fn set_context(&self, _patch: &TelemetryContextPatch) {}

    fn add_appender(&self, _appender: Arc<dyn TelemetryAppender>) -> DisposableHandle {
        disposable_none()
    }

    fn remove_appender(&self, _appender: &Arc<dyn TelemetryAppender>) {}

    fn set_appender(&self, _appender: Arc<dyn TelemetryAppender>) {}

    fn set_enabled(&self, _enabled: bool) {}

    async fn flush(&self) {}

    async fn shutdown(&self) {}
}

pub fn null_telemetry_appender() -> Arc<dyn TelemetryAppender> {
    Arc::new(NullTelemetryAppender)
}

pub fn noop_telemetry_service() -> TelemetryServiceHandle {
    TelemetryServiceHandle(Arc::new(NoopTelemetryService))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn null_appender_and_noop_service_accept_the_complete_lifecycle() {
        let appender = null_telemetry_appender();
        let properties = TelemetryProperties::from([("missing".into(), None)]);
        appender.track("event", Some(&properties));
        assert!(appender.with_context(&properties).is_some());
        appender.flush().await.unwrap();
        appender.shutdown().await.unwrap();

        let service = noop_telemetry_service();
        service.track("event", Some(&properties));
        service.set_context(&properties);
        let child = service.with_context(&properties);
        child.track("child", None);
        service.set_enabled(false);
        service.flush().await;
        service.shutdown().await;
        assert_eq!(TELEMETRY_SERVICE_ID.to_string(), "agentTelemetryService");
    }
}
