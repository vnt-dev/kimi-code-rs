//! Agent-scoped telemetry context implementation.
//!
//! Original:
//! `packages/agent-core-v2/src/app/telemetry/agentTelemetryContextService.ts`.

use parking_lot::Mutex;
use std::sync::Arc;

use crate::_base::di::{
    descriptors::SyncDescriptor,
    scope::{InstantiationType, LifecycleScope, register_scoped_service},
};

use super::agent_telemetry_context::{
    AGENT_TELEMETRY_CONTEXT_SERVICE_ID, AgentTelemetryContext, AgentTelemetryContextPatch,
    AgentTelemetryContextServiceContract, AgentTelemetryContextServiceHandle,
};

pub struct AgentTelemetryContextService {
    context: Mutex<AgentTelemetryContext>,
}

impl Default for AgentTelemetryContextService {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentTelemetryContextService {
    // Original: AgentTelemetryContextService.constructor().
    pub fn new() -> Self {
        Self {
            context: Mutex::new(AgentTelemetryContext::default()),
        }
    }
}

impl AgentTelemetryContextServiceContract for AgentTelemetryContextService {
    // Original: AgentTelemetryContextService.get(). Returning an owned clone
    // preserves the source snapshot: set() replaces its context object rather
    // than mutating previously returned objects.
    fn get(&self) -> AgentTelemetryContext {
        self.context.lock().clone()
    }

    // Original: AgentTelemetryContextService.set().
    fn set(&self, patch: AgentTelemetryContextPatch) {
        let mut context = self.context.lock();
        if let Some(mode) = patch.mode {
            context.mode = mode;
        }
        if let Some(provider_type) = patch.provider_type {
            context.provider_type = provider_type;
        }
        if let Some(protocol) = patch.protocol {
            context.protocol = protocol;
        }
        if let Some(turn_id) = patch.turn_id {
            context.turn_id = turn_id;
        }
        if let Some(trace_id) = patch.trace_id {
            context.trace_id = trace_id;
        }
    }
}

// Original: registerScopedService(... AgentTelemetryContextService ...).
pub fn register_agent_telemetry_context_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_TELEMETRY_CONTEXT_SERVICE_ID,
        SyncDescriptor::new(|_| {
            let service: Arc<dyn AgentTelemetryContextServiceContract> =
                Arc::new(AgentTelemetryContextService::new());
            Ok(AgentTelemetryContextServiceHandle(service))
        }),
        InstantiationType::Eager,
        "telemetry",
    );
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use crate::app::telemetry::AgentTelemetryMode;

    use super::*;

    #[test]
    fn defaults_merges_clears_and_returns_context_snapshots() {
        let context = AgentTelemetryContextService::new();
        let initial = context.get();
        assert_eq!(initial.mode, AgentTelemetryMode::Agent);

        context.set(AgentTelemetryContextPatch {
            mode: Some(AgentTelemetryMode::Plan),
            provider_type: Some(Some("kimi".into())),
            trace_id: Some(Some("trace-1".into())),
            ..AgentTelemetryContextPatch::default()
        });
        let plan = context.get();
        assert_eq!(initial.mode, AgentTelemetryMode::Agent);
        assert_eq!(plan.mode, AgentTelemetryMode::Plan);
        assert_eq!(plan.provider_type.as_deref(), Some("kimi"));

        context.set(AgentTelemetryContextPatch {
            provider_type: Some(None),
            ..AgentTelemetryContextPatch::default()
        });
        let cleared = context.get();
        assert_eq!(cleared.provider_type, None);
        assert_eq!(cleared.trace_id.as_deref(), Some("trace-1"));

        context.set(AgentTelemetryContextPatch {
            mode: Some(AgentTelemetryMode::Agent),
            ..AgentTelemetryContextPatch::default()
        });
        let properties = plan.to_telemetry_properties();
        assert_eq!(properties["mode"], Some(Value::from("plan")));
    }
}
