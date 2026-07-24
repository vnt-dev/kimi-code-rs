//! Tool-result continuation aspect for the agent loop.
//!
//! Original: `packages/agent-core-v2/src/agent/loop/loopContinuation.ts` and
//! `loopContinuationService.ts`.

use std::{ops::Deref, sync::Arc};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::{ServiceIdentifier, ServicesAccessorExt},
        lifecycle::{Disposable, DisposableStore, DisposeResult},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    hooks::HookRegisterOptions,
    kosong::contract::provider::FinishReason,
};

use super::{
    AGENT_LOOP_SERVICE_ID, AgentLoopServiceContract, ContinuationStepRequest,
    MessageStepRequestOptions,
};

pub trait AgentLoopContinuationServiceContract: Disposable + Send + Sync {}

#[derive(Clone)]
pub struct AgentLoopContinuationServiceHandle(pub Arc<dyn AgentLoopContinuationServiceContract>);
impl Deref for AgentLoopContinuationServiceHandle {
    type Target = dyn AgentLoopContinuationServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
impl Disposable for AgentLoopContinuationServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}
pub const AGENT_LOOP_CONTINUATION_SERVICE_ID: ServiceIdentifier<
    AgentLoopContinuationServiceHandle,
> = ServiceIdentifier::new("agentLoopContinuationService");

pub struct AgentLoopContinuationService {
    disposables: DisposableStore,
}
impl AgentLoopContinuationService {
    pub fn new(
        loop_service: Arc<dyn AgentLoopServiceContract>,
    ) -> Result<Self, crate::hooks::HookRegistrationError> {
        let disposables = DisposableStore::new();
        let loop_for_hook = Arc::clone(&loop_service);
        let hook = loop_service.hooks().on_did_finish_step.register(
            "loop-continuation",
            Arc::new(move |context, next| {
                let loop_service = Arc::clone(&loop_for_hook);
                Box::pin(async move {
                    next(context).await?;
                    if !context.stop_turn && context.finish_reason == FinishReason::ToolCalls {
                        loop_service
                            .enqueue(
                                Arc::new(ContinuationStepRequest::new(
                                    MessageStepRequestOptions::default(),
                                )),
                                None,
                            )
                            .map_err(Box::new)?;
                    }
                    Ok(())
                })
            }),
            HookRegisterOptions::default(),
        )?;
        disposables.add(hook);
        Ok(Self { disposables })
    }
}
impl AgentLoopContinuationServiceContract for AgentLoopContinuationService {}
impl Disposable for AgentLoopContinuationService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}
pub fn register_agent_loop_continuation_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_LOOP_CONTINUATION_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let loop_service = accessor.get(AGENT_LOOP_SERVICE_ID)?;
            let service: Arc<dyn AgentLoopContinuationServiceContract> = Arc::new(
                AgentLoopContinuationService::new(Arc::clone(&loop_service.0)).map_err(
                    |error| crate::_base::di::errors::DiError::Factory(error.to_string()),
                )?,
            );
            Ok(AgentLoopContinuationServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "loop",
    );
}
