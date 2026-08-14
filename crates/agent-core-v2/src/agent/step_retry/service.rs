//! Agent-scoped step-retry loop plugin.
//!
//! Original: `stepRetryService.ts`.

use std::ops::Deref;
use std::sync::{Arc, Weak};
use parking_lot::Mutex;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            errors::DiError,
            instantiation::{ServiceIdentifier, ServicesAccessorExt},
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::{ErrorCauseRef, unwrap_error_cause},
        utils::retry::{
            DEFAULT_MAX_RETRY_ATTEMPTS, read_retry_after_ms, retry_backoff_delays,
            retry_error_fields, sleep_for_retry,
        },
    },
    agent::loop_::{
        AGENT_LOOP_SERVICE_ID, AgentLoopServiceHandle, LoopErrorContext, LoopErrorHandler,
        LoopErrorHandlerRegistrationOptions, LoopValue, StepEnqueueOptions,
        StepRequestQueuePosition,
    },
    app::{
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
        event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID, EventBusHandle},
    },
    hooks::HookRegisterOptions,
    kosong::contract::errors::{ChatProviderError, is_retryable_generate_error},
    session::question::{
        QuestionAnswer, QuestionItem, QuestionOption, QuestionPresentation, QuestionRequest,
        QuestionRequestOptions, QuestionResult, SESSION_QUESTION_SERVICE_ID,
        SessionQuestionServiceHandle,
    },
};

use crate::agent::loop_::{LOOP_CONTROL_SECTION, LoopControl};

pub trait AgentStepRetryServiceContract: Disposable + Send + Sync {}

#[derive(Clone)]
pub struct AgentStepRetryServiceHandle(pub Arc<dyn AgentStepRetryServiceContract>);
impl Deref for AgentStepRetryServiceHandle {
    type Target = dyn AgentStepRetryServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
impl Disposable for AgentStepRetryServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}
pub const AGENT_STEP_RETRY_SERVICE_ID: ServiceIdentifier<AgentStepRetryServiceHandle> =
    ServiceIdentifier::new("agentStepRetryService");

#[derive(Default)]
struct RetryState {
    last_failed_driver_id: Option<String>,
    failed_attempts: u64,
}

pub struct AgentStepRetryService {
    config: ConfigServiceHandle,
    event_bus: EventBusHandle,
    question: SessionQuestionServiceHandle,
    state: Mutex<RetryState>,
    disposables: DisposableStore,
}

impl AgentStepRetryService {
    pub fn new(
        config: ConfigServiceHandle,
        event_bus: EventBusHandle,
        loop_service: AgentLoopServiceHandle,
        question: SessionQuestionServiceHandle,
    ) -> Result<Arc<Self>, crate::hooks::HookRegistrationError> {
        let service = Arc::new(Self {
            config,
            event_bus,
            question,
            state: Mutex::new(RetryState::default()),
            disposables: DisposableStore::new(),
        });
        service.install(loop_service)?;
        Ok(service)
    }

    fn install(
        self: &Arc<Self>,
        loop_service: AgentLoopServiceHandle,
    ) -> Result<(), crate::hooks::HookRegistrationError> {
        let handler: Arc<dyn LoopErrorHandler> = Arc::new(StepRetryHandler {
            service: Arc::downgrade(self),
        });
        let disposable = loop_service
            .register_loop_error_handler(handler, LoopErrorHandlerRegistrationOptions::default())
            .map_err(|error| {
                crate::hooks::HookRegistrationError::TargetNotRegistered(error.to_string())
            })?;
        self.disposables.add(disposable);
        let weak = Arc::downgrade(self);
        self.disposables
            .add(loop_service.hooks().on_did_finish_step.register(
                "step-retry",
                Arc::new(move |context, next| {
                    let weak = Weak::clone(&weak);
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade() {
                            service.reset_attempts();
                        }
                        next(context).await
                    })
                }),
                HookRegisterOptions::default(),
            )?);
        let weak = Arc::downgrade(self);
        self.disposables.add(self.event_bus.subscribe_type(
            "turn.started",
            Arc::new(move |_| {
                if let Some(service) = weak.upgrade() {
                    service.reset_attempts();
                }
            }),
        ));
        Ok(())
    }

    fn reset_attempts(&self) {
        *self.state.lock() = RetryState::default();
    }

    fn max_attempts(&self) -> u64 {
        self.config
            .get(LOOP_CONTROL_SECTION)
            .and_then(|value| serde_json::from_value::<LoopControl>(value).ok())
            .and_then(|value| value.max_retries_per_step)
            .unwrap_or(DEFAULT_MAX_RETRY_ATTEMPTS as u64)
            .max(1)
    }

    async fn recover(&self, context: &mut LoopErrorContext) -> Result<Option<bool>, LoopValue> {
        let Some(driver) = context.failed_driver.clone() else {
            return Ok(Some(false));
        };
        let Some(step) = context.step else {
            return Ok(Some(false));
        };
        let failed_attempt = {
            let mut state = self.state.lock();
            if state.last_failed_driver_id.as_deref() != Some(driver.id()) {
                state.last_failed_driver_id = Some(driver.id().into());
                state.failed_attempts = 0;
            }
            state.failed_attempts += 1;
            state.failed_attempts
        };
        let max_attempts = self.max_attempts();
        if failed_attempt >= max_attempts {
            // Once the automatic budget is exhausted, every further failure
            // requires a fresh confirmation. Keep the counter on approval so
            // a failed user-approved attempt does not restart silent retries.
            let error_message = underlying_error(&context.error)
                .unwrap_or(&context.error)
                .to_string();
            if !self
                .confirm_retry(context, step, failed_attempt, &error_message)
                .await
            {
                self.reset_attempts();
                if let Some(reason) = context.signal.reason() {
                    return Err(LoopValue::Error(reason));
                }
                return Ok(Some(false));
            }
        }
        let error = underlying_error(&context.error).unwrap_or(&context.error);
        let delay_ms = if failed_attempt >= max_attempts {
            0
        } else {
            read_retry_after_ms(error).unwrap_or_else(|| {
                retry_backoff_delays(max_attempts as usize)
                    .get((failed_attempt - 1) as usize)
                    .copied()
                    .unwrap_or(0)
            })
        };
        self.publish_retry(
            context,
            step,
            failed_attempt,
            max_attempts.max(failed_attempt + 1),
            delay_ms,
            error,
        )?;
        if sleep_for_retry(delay_ms, Some(&context.signal))
            .await
            .is_err()
        {
            return Ok(Some(false));
        }
        if context
            .current_step
            .as_ref()
            .is_some_and(|step| step.0.signal().aborted())
        {
            return Ok(Some(false));
        }
        (context.retry)(
            driver,
            Some(StepEnqueueOptions {
                at: Some(StepRequestQueuePosition::Head),
            }),
        );
        Ok(Some(true))
    }

    async fn confirm_retry(
        &self,
        context: &LoopErrorContext,
        step: crate::agent::StepId,
        failed_attempt: u64,
        error_message: &str,
    ) -> bool {
        let result = self
            .question
            .request(
                retry_confirmation_request(context, step, failed_attempt, error_message),
                Some(QuestionRequestOptions {
                    signal: Some(context.signal.clone()),
                    agent_id: None,
                }),
            )
            .await;
        retry_was_confirmed(result)
    }

    fn publish_retry(
        &self,
        context: &LoopErrorContext,
        step: crate::agent::StepId,
        failed_attempt: u64,
        max_attempts: u64,
        delay_ms: u64,
        error: &(dyn std::error::Error + 'static),
    ) -> Result<(), LoopValue> {
        let fields = RetryEvent {
            turn_id: context.turn_id,
            step,
            step_id: context.step_id.clone(),
            failed_attempt,
            next_attempt: failed_attempt + 1,
            max_attempts,
            delay_ms,
            fields: retry_error_fields(error),
        };
        let Value::Object(fields) =
            serde_json::to_value(fields).map_err(|error| LoopValue::Error(Arc::new(error)))?
        else {
            return Ok(());
        };
        self.event_bus
            .publish(DomainEvent::new("turn.step.retrying", fields));
        Ok(())
    }
}

struct StepRetryHandler {
    service: Weak<AgentStepRetryService>,
}
#[async_trait]
impl LoopErrorHandler for StepRetryHandler {
    fn id(&self) -> &str {
        "step-retry"
    }
    fn matches(&self, context: &LoopErrorContext) -> bool {
        underlying_error(&context.error)
            .and_then(|error| error.downcast_ref::<ChatProviderError>())
            .is_some_and(is_retryable_generate_error)
    }
    async fn handle(&self, context: &mut LoopErrorContext) -> Result<Option<bool>, LoopValue> {
        match self.service.upgrade() {
            Some(service) => service.recover(context).await,
            None => Ok(Some(false)),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RetryEvent {
    turn_id: crate::agent::TurnId,
    step: crate::agent::StepId,
    #[serde(skip_serializing_if = "Option::is_none")]
    step_id: Option<String>,
    failed_attempt: u64,
    next_attempt: u64,
    max_attempts: u64,
    delay_ms: u64,
    #[serde(flatten)]
    fields: crate::_base::utils::retry::RetryErrorFields,
}

fn underlying_error(error: &LoopValue) -> Option<&(dyn std::error::Error + 'static)> {
    let LoopValue::Error(error) = error else {
        return None;
    };
    match unwrap_error_cause(error.as_ref()) {
        ErrorCauseRef::Error(error) => Some(error),
        ErrorCauseRef::Value(_) => None,
    }
}

impl AgentStepRetryServiceContract for AgentStepRetryService {}
impl Disposable for AgentStepRetryService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

pub fn register_agent_step_retry_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_STEP_RETRY_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let loop_service: AgentLoopServiceHandle =
                (*accessor.get(AGENT_LOOP_SERVICE_ID)?).clone();
            let config: ConfigServiceHandle = (*accessor.get(CONFIG_SERVICE_ID)?).clone();
            let event_bus: EventBusHandle = (*accessor.get(EVENT_BUS_SERVICE_ID)?).clone();
            let question: SessionQuestionServiceHandle =
                (*accessor.get(SESSION_QUESTION_SERVICE_ID)?).clone();
            let service = AgentStepRetryService::new(config, event_bus, loop_service, question)
                .map_err(|error| DiError::Factory(error.to_string()))?;
            Ok(AgentStepRetryServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "stepRetry",
    );
}

const RETRY_CONFIRMATION_QUESTION: &str = "The retry failed again. Continue?";
const RETRY_CONFIRMATION_LABEL: &str = "Retry";

fn retry_confirmation_request(
    context: &LoopErrorContext,
    step: crate::agent::StepId,
    failed_attempt: u64,
    error_message: &str,
) -> QuestionRequest {
    let step_id = context.step_id.as_deref().unwrap_or("unknown");
    QuestionRequest {
        id: Some(format!(
            "step-retry:{}:{step}:{step_id}:{failed_attempt}",
            context.turn_id
        )),
        turn_id: Some(context.turn_id),
        tool_call_id: None,
        presentation: Some(QuestionPresentation::RetryConfirmation),
        questions: vec![QuestionItem {
            question: RETRY_CONFIRMATION_QUESTION.into(),
            header: Some("Model request failed".into()),
            body: Some(format!(
                "The automatic retry limit was reached after {failed_attempt} attempts. Last error: {error_message}"
            )),
            options: vec![
                QuestionOption {
                    label: RETRY_CONFIRMATION_LABEL.into(),
                    description: Some(
                        "Continue this turn and try the same model request again.".into(),
                    ),
                },
                QuestionOption {
                    label: "Stop".into(),
                    description: Some("Stop retrying and end this turn as failed.".into()),
                },
            ],
            multi_select: Some(false),
            other_label: None,
            other_description: None,
        }],
    }
}

fn retry_was_confirmed(result: Option<QuestionResult>) -> bool {
    let answers = match result {
        Some(QuestionResult::Answers(answers)) => answers,
        Some(QuestionResult::Response(response)) => response.answers,
        None => return false,
    };
    answers.values().any(
        |answer| matches!(answer, QuestionAnswer::Text(label) if label == RETRY_CONFIRMATION_LABEL),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::session::question::{QuestionAnswerMethod, QuestionResponse};

    #[test]
    fn only_the_retry_answer_confirms_an_extra_attempt() {
        let result = |label: &str| {
            Some(QuestionResult::Response(QuestionResponse {
                answers: HashMap::from([(
                    RETRY_CONFIRMATION_QUESTION.into(),
                    QuestionAnswer::Text(label.into()),
                )]),
                method: Some(QuestionAnswerMethod::Enter),
            }))
        };

        assert!(retry_was_confirmed(result(RETRY_CONFIRMATION_LABEL)));
        assert!(!retry_was_confirmed(result("Stop")));
        assert!(!retry_was_confirmed(None));
    }
}
