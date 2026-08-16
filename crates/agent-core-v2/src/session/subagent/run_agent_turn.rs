//! Run one prompt (or retry) turn in a target agent scope.
//!
//! Original: `packages/agent-core-v2/src/session/subagent/runAgentTurn.ts`,
//! `runAgentTurn()`, `awaitRun()`, `awaitTurn()`, and `distillSummary()`.
//!
//! Rust adaptation: `ScopeHandle` provides typed service access in place of
//! the TypeScript scope accessor.  Waiting remains asynchronous; importantly,
//! cancellation first requests loop cancellation and then still awaits the
//! terminal turn result so the loop has released its active turn.

use std::{error::Error, fmt, sync::Arc};

use futures_util::FutureExt;
use serde_json::Value;

use crate::{
    _base::{
        di::{errors::DiError, scope::ScopeHandle},
        errors::errors::Error2,
        lifecycle::lifecycle_machine::BoxError,
        utils::abort::{AbortController, AbortSignal, link_abort_signal},
    },
    agent::{
        context_memory::{
            AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceContract, ContextMessage,
            PromptOrigin,
        },
        loop_::{
            AGENT_LOOP_SERVICE_ID, AgentLoopServiceContract, LoopRunResult, LoopValue, TurnHandle,
        },
        prompt::{AGENT_PROMPT_SERVICE_ID, AgentPromptServiceContract, PromptInput},
        usage::{AGENT_USAGE_SERVICE_ID, AgentUsageServiceContract},
    },
    kosong::{
        contract::{
            errors::{ApiStatusData, ChatProviderError, is_provider_rate_limit_error},
            message::{ContentPart, Message, Role},
        },
        protocol::errors::PROVIDER_RATE_LIMIT,
    },
};

use super::{
    AgentRunCompletion, AgentRunCompletionFuture, AgentRunHandle, AgentRunRequest, RunAgentOptions,
};

pub const SUBAGENT_MAX_TOKENS_ERROR: &str =
    "Subagent turn failed before completing its final summary: reason=max_tokens";

/// Original `AGENT_RUN_PROMPT_ORIGIN`.
pub fn agent_run_prompt_origin() -> PromptOrigin {
    PromptOrigin::SystemTrigger {
        name: "subagent".into(),
    }
}

#[derive(Clone)]
struct AgentRunServices {
    prompt: Arc<dyn AgentPromptServiceContract>,
    loop_service: Arc<dyn AgentLoopServiceContract>,
    memory: Arc<dyn AgentContextMemoryServiceContract>,
    usage: Option<Arc<dyn AgentUsageServiceContract>>,
}

/// Original `runAgentTurn()`.
pub async fn run_agent_turn(
    target: &ScopeHandle,
    request: AgentRunRequest,
    options: RunAgentOptions,
) -> Result<AgentRunHandle, BoxError> {
    let services = services_from_scope(target)?;
    run_agent_turn_with_services(target.id().to_owned(), services, request, options).await
}

fn services_from_scope(target: &ScopeHandle) -> Result<AgentRunServices, BoxError> {
    let prompt = target.get(AGENT_PROMPT_SERVICE_ID)?.0.clone();
    let loop_service = target.get(AGENT_LOOP_SERVICE_ID)?.0.clone();
    let memory = target.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?.0.clone();
    // The source treats usage as optional. An absent registration therefore
    // yields `undefined` rather than preventing a completed subagent run.
    let usage = match target.get(AGENT_USAGE_SERVICE_ID) {
        Ok(handle) => Some(handle.0.clone()),
        Err(DiError::UnknownService(_)) => None,
        Err(error) => return Err(Box::new(error)),
    };
    Ok(AgentRunServices {
        prompt,
        loop_service,
        memory,
        usage,
    })
}

async fn run_agent_turn_with_services(
    agent_id: String,
    services: AgentRunServices,
    request: AgentRunRequest,
    options: RunAgentOptions,
) -> Result<AgentRunHandle, BoxError> {
    options
        .signal
        .throw_if_aborted()
        .map_err(|error| Box::new((*error).clone()) as BoxError)?;

    let turn = match request {
        AgentRunRequest::Prompt { prompt } => {
            let handle = services
                .prompt
                .enqueue(PromptInput {
                    id: None,
                    message: user_message(prompt),
                })
                .await?;
            handle.launched().await
        }
        AgentRunRequest::Retry { .. } => services.prompt.retry().await?,
    }
    .ok_or_else(|| Box::new(AgentTurnNotStarted) as BoxError)?;

    let completion: AgentRunCompletionFuture = await_run(services, turn.clone(), options)
        .map(|result| result.map_err(Arc::from))
        .boxed()
        .shared();
    Ok(AgentRunHandle {
        agent_id,
        turn,
        completion,
    })
}

fn user_message(text: String) -> ContextMessage {
    ContextMessage {
        message: Message::new(Role::User, vec![ContentPart::Text { text }], Vec::new()),
        id: None,
        provider_message_id: None,
        origin: Some(agent_run_prompt_origin()),
        is_error: None,
        note: None,
        attachments: Vec::new(),
    }
}

async fn await_run(
    services: AgentRunServices,
    first_turn: TurnHandle,
    options: RunAgentOptions,
) -> Result<AgentRunCompletion, BoxError> {
    let controller = AbortController::new();
    let signal = controller.signal();
    let mut link = link_abort_signal(&options.signal, controller);
    let mut turn = first_turn;
    let mut first_ready = options.on_ready;

    let result = async {
        let result = await_turn(&services.loop_service, &turn, &signal, first_ready.take()).await?;
        classify_turn_result(result)?;

        let summary = distill_summary(
            &services,
            &signal,
            options.summary_policy.as_ref(),
            &mut turn,
        )
        .await?;
        let usage = services
            .usage
            .as_ref()
            .and_then(|usage| usage.status().total);
        Ok(AgentRunCompletion { summary, usage })
    }
    .await;

    link.unlink();
    if let Some(reason) = signal.reason() {
        cancel_turn(&services.loop_service, &turn, reason);
    }
    result
}

/// Original `awaitTurn()`. This never races the terminal result with abort:
/// abort sends cancellation, then the method remains pending until the loop's
/// turn has settled and released its active state.
async fn await_turn(
    loop_service: &Arc<dyn AgentLoopServiceContract>,
    turn: &TurnHandle,
    signal: &AbortSignal,
    on_ready: Option<Arc<dyn Fn() + Send + Sync>>,
) -> Result<LoopRunResult, BoxError> {
    let mut ready_pending = on_ready.is_some();
    let mut cancellation_sent = false;
    loop {
        if !cancellation_sent && let Some(reason) = signal.reason() {
            cancel_turn(loop_service, turn, reason);
            cancellation_sent = true;
        }
        tokio::select! {
            result = turn.0.result() => {
                signal
                    .throw_if_aborted()
                    .map_err(|error| Box::new((*error).clone()) as BoxError)?;
                return Ok(result);
            }
            ready = turn.0.ready(), if ready_pending => {
                ready_pending = false;
                if ready.is_ok() && let Some(on_ready) = on_ready.as_ref() {
                    on_ready();
                }
            }
            reason = signal.cancelled(), if !cancellation_sent => {
                cancel_turn(loop_service, turn, reason);
                cancellation_sent = true;
            }
        }
    }
}

/// Original `distillSummary()`.
async fn distill_summary(
    services: &AgentRunServices,
    signal: &AbortSignal,
    policy: Option<&crate::app::agent_profile_catalog::AgentProfileSummaryPolicy>,
    turn: &mut TurnHandle,
) -> Result<String, BoxError> {
    let mut summary = latest_assistant_text(&services.memory.get());
    let Some(policy) = policy else {
        return Ok(summary);
    };
    if is_summary_adequate(&summary, policy) {
        return Ok(summary);
    }

    for _ in 0..policy.retries {
        let handle = services
            .prompt
            .enqueue(PromptInput {
                id: None,
                message: user_message(policy.continuation_prompt.clone()),
            })
            .await?;
        let Some(next_turn) = handle.launched().await else {
            break;
        };
        *turn = next_turn;
        let result = await_turn(&services.loop_service, turn, signal, None).await?;
        classify_turn_result(result)?;
        let continued = latest_assistant_text(&services.memory.get());
        if !continued.trim().is_empty() {
            summary = continued;
        }
        if is_summary_adequate(&summary, policy) {
            break;
        }
    }
    Ok(summary)
}

fn is_summary_adequate(
    summary: &str,
    policy: &crate::app::agent_profile_catalog::AgentProfileSummaryPolicy,
) -> bool {
    summary.trim().len() >= policy.min_chars
}

fn cancel_turn(
    loop_service: &Arc<dyn AgentLoopServiceContract>,
    turn: &TurnHandle,
    reason: Arc<crate::_base::utils::abort::AbortError>,
) {
    let reason: Arc<dyn Error + Send + Sync> = reason;
    loop_service.cancel(Some(turn.0.id()), Some(LoopValue::Error(reason)));
}

/// Original `classifyTurnResult()`.
fn classify_turn_result(result: LoopRunResult) -> Result<(), BoxError> {
    match result {
        LoopRunResult::Completed {
            truncated: true, ..
        } => Err(Box::new(SubagentMaxTokensError)),
        LoopRunResult::Completed { .. } => Ok(()),
        LoopRunResult::Failed { error, .. } => Err(loop_value_to_error(error)),
        LoopRunResult::Cancelled { reason, .. } => Err(loop_value_to_error(reason)),
    }
}

fn loop_value_to_error(value: LoopValue) -> BoxError {
    match value {
        LoopValue::Error(error) => {
            if let Some(provider) = error.downcast_ref::<ChatProviderError>()
                && is_provider_rate_limit_error(provider)
            {
                return Box::new(provider.clone());
            }
            if let Some(error2) = error.downcast_ref::<Error2>() {
                if error2.code == PROVIDER_RATE_LIMIT {
                    let request_id = error2
                        .details
                        .as_ref()
                        .and_then(|details| details.get("requestId"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    return Box::new(ChatProviderError::ApiProviderRateLimit {
                        message: error2.message.clone(),
                        data: ApiStatusData::new(429, request_id, None, None),
                    });
                }
                return Box::new(error2.clone());
            }
            Box::new(SharedRunError(error))
        }
        LoopValue::Value(value) => Box::new(RunValueError(stringify_run_value(&value))),
    }
}

fn stringify_run_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}

fn latest_assistant_text(messages: &[ContextMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.message.role == Role::Assistant)
        .map(|message| content_text(&message.message.content))
        .unwrap_or_default()
}

fn content_text(content: &[ContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[derive(Debug, thiserror::Error)]
#[error("Agent turn could not be started")]
struct AgentTurnNotStarted;

#[derive(Debug, thiserror::Error)]
#[error("{SUBAGENT_MAX_TOKENS_ERROR}")]
struct SubagentMaxTokensError;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct RunValueError(String);

struct SharedRunError(Arc<dyn Error + Send + Sync>);

impl fmt::Debug for SharedRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SharedRunError")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SharedRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for SharedRunError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        _base::errors::errors::Error2, agent::context_memory::PromptOrigin,
        kosong::contract::errors::ApiStatusData,
    };

    fn context_message(role: Role, content: Vec<ContentPart>) -> ContextMessage {
        ContextMessage {
            message: Message::new(role, content, Vec::new()),
            id: None,
            provider_message_id: None,
            origin: None,
            is_error: None,
            note: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn prompt_messages_and_summary_text_match_source_rules() {
        let message = user_message("delegate this".into());
        assert_eq!(message.message.role, Role::User);
        assert_eq!(
            message.message.content.as_ref(),
            &[ContentPart::Text {
                text: "delegate this".into()
            }]
        );
        assert_eq!(
            message.origin,
            Some(PromptOrigin::SystemTrigger {
                name: "subagent".into()
            })
        );

        let messages = vec![
            context_message(
                Role::Assistant,
                vec![ContentPart::Text { text: "old".into() }],
            ),
            context_message(
                Role::User,
                vec![ContentPart::Text {
                    text: "ignore".into(),
                }],
            ),
            context_message(
                Role::Assistant,
                vec![
                    ContentPart::Think {
                        think: "hidden".into(),
                        encrypted: None,
                    },
                    ContentPart::Text {
                        text: "final".into(),
                    },
                    ContentPart::Text {
                        text: " summary".into(),
                    },
                ],
            ),
        ];
        assert_eq!(latest_assistant_text(&messages), "final summary");
    }

    #[test]
    fn result_classification_preserves_truncation_and_rate_limit_cases() {
        let truncated = classify_turn_result(LoopRunResult::Completed {
            steps: 1,
            truncated: true,
        })
        .unwrap_err();
        assert_eq!(truncated.to_string(), SUBAGENT_MAX_TOKENS_ERROR);

        let rate_limit = ChatProviderError::ApiProviderRateLimit {
            message: "slow down".into(),
            data: ApiStatusData::new(429, Some("request-1".into()), None, None),
        };
        let error = classify_turn_result(LoopRunResult::Failed {
            steps: 1,
            error: LoopValue::Error(Arc::new(rate_limit)),
        })
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ChatProviderError>(),
            Some(ChatProviderError::ApiProviderRateLimit { .. })
        ));

        let error = classify_turn_result(LoopRunResult::Failed {
            steps: 1,
            error: LoopValue::Error(Arc::new(Error2::new(PROVIDER_RATE_LIMIT, "back off"))),
        })
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<ChatProviderError>(),
            Some(ChatProviderError::ApiProviderRateLimit { .. })
        ));
    }

    #[test]
    fn summary_policy_uses_trimmed_character_count() {
        let policy = crate::app::agent_profile_catalog::AgentProfileSummaryPolicy {
            min_chars: 4,
            continuation_prompt: "continue".into(),
            retries: 1,
        };
        assert!(!is_summary_adequate("  ok  ", &policy));
        assert!(is_summary_adequate("  done  ", &policy));
        assert_eq!(
            stringify_run_value(&Value::String("failed".into())),
            "failed"
        );
    }
}
