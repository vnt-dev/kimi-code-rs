//! Agent-scoped mutable telemetry request context contract.
//!
//! Original: `packages/agent-core-v2/src/app/telemetry/agentTelemetryContext.ts`.

use std::{ops::Deref, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::_base::di::instantiation::ServiceIdentifier;

use super::contract::TelemetryProperties;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTelemetryMode {
    Agent,
    Plan,
}

impl AgentTelemetryMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Plan => "plan",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentTelemetryContext {
    pub mode: AgentTelemetryMode,
    pub provider_type: Option<String>,
    pub protocol: Option<String>,
    pub turn_id: Option<u64>,
    pub trace_id: Option<String>,
}

impl Default for AgentTelemetryContext {
    fn default() -> Self {
        Self {
            mode: AgentTelemetryMode::Agent,
            provider_type: None,
            protocol: None,
            turn_id: None,
            trace_id: None,
        }
    }
}

impl AgentTelemetryContext {
    /// Returns the structural telemetry object accepted directly by the
    /// TypeScript `withContext()` call.
    pub fn to_telemetry_properties(&self) -> TelemetryProperties {
        let mut properties =
            TelemetryProperties::from([("mode".into(), Some(Value::from(self.mode.as_str())))]);
        extend_optional_string(&mut properties, "provider_type", &self.provider_type);
        extend_optional_string(&mut properties, "protocol", &self.protocol);
        if let Some(turn_id) = self.turn_id {
            properties.insert("turn_id".into(), Some(Value::from(turn_id)));
        }
        extend_optional_string(&mut properties, "trace_id", &self.trace_id);
        properties
    }
}

fn extend_optional_string(properties: &mut TelemetryProperties, key: &str, value: &Option<String>) {
    if let Some(value) = value {
        properties.insert(key.into(), Some(Value::from(value.clone())));
    }
}

/// Rust counterpart of `Partial<AgentTelemetryContext>`.
///
/// Optional source fields use two `Option` layers: the outer layer says
/// whether the patch contains the property, while the inner layer preserves
/// an explicit JavaScript `undefined` as a request to clear its current value.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentTelemetryContextPatch {
    pub mode: Option<AgentTelemetryMode>,
    pub provider_type: Option<Option<String>>,
    pub protocol: Option<Option<String>>,
    pub turn_id: Option<Option<u64>>,
    pub trace_id: Option<Option<String>>,
}

pub trait AgentTelemetryContextServiceContract: Send + Sync {
    fn get(&self) -> AgentTelemetryContext;
    fn set(&self, patch: AgentTelemetryContextPatch);
}

#[derive(Clone)]
pub struct AgentTelemetryContextServiceHandle(pub Arc<dyn AgentTelemetryContextServiceContract>);

impl Deref for AgentTelemetryContextServiceHandle {
    type Target = dyn AgentTelemetryContextServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_TELEMETRY_CONTEXT_SERVICE_ID: ServiceIdentifier<
    AgentTelemetryContextServiceHandle,
> = ServiceIdentifier::new("agentTelemetryContextService");
