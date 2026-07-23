use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::{
    _base::{di::instantiation::ServiceIdentifier, utils::abort::AbortSignal},
    agent::external_hooks::{HookBlockDecision, HookMatcherValue, HookResult},
};

#[derive(Clone, Default)]
pub struct ExternalHooksRunnerTriggerArgs {
    pub matcher_value: Option<HookMatcherValue>,
    pub input_data: Option<Map<String, Value>>,
    pub signal: Option<AbortSignal>,
    pub cwd: Option<String>,
    pub session_id: Option<String>,
}

#[async_trait]
pub trait ExternalHooksRunnerServiceContract: Send + Sync {
    async fn trigger(&self, event: &str, args: ExternalHooksRunnerTriggerArgs) -> Vec<HookResult>;

    async fn trigger_block(
        &self,
        event: &str,
        args: ExternalHooksRunnerTriggerArgs,
    ) -> Option<HookBlockDecision>;

    async fn fire_and_forget_trigger(
        &self,
        event: &str,
        args: ExternalHooksRunnerTriggerArgs,
    ) -> Vec<HookResult>;
}

#[derive(Clone)]
pub struct ExternalHooksRunnerServiceHandle(pub Arc<dyn ExternalHooksRunnerServiceContract>);

impl Deref for ExternalHooksRunnerServiceHandle {
    type Target = dyn ExternalHooksRunnerServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

// Original:
//   packages/agent-core-v2/src/app/externalHooksRunner/externalHooksRunner.ts
//   IExternalHooksRunnerService
pub const EXTERNAL_HOOKS_RUNNER_SERVICE_ID: ServiceIdentifier<ExternalHooksRunnerServiceHandle> =
    ServiceIdentifier::new("externalHooksRunnerService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_defaults_and_service_identity_match_source() {
        let args = ExternalHooksRunnerTriggerArgs::default();
        assert!(args.matcher_value.is_none());
        assert!(args.input_data.is_none());
        assert!(args.signal.is_none());
        assert!(args.cwd.is_none());
        assert!(args.session_id.is_none());
        assert_eq!(
            EXTERNAL_HOOKS_RUNNER_SERVICE_ID.to_string(),
            "externalHooksRunnerService"
        );
    }
}
