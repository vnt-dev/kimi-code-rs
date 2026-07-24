//! Session swarm batch scheduler contracts and deterministic scheduling helpers.
//!
//! Original: `session/swarm/agentRunBatch.ts`.

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures_util::future::BoxFuture;

use crate::{
    _base::{lifecycle::lifecycle_machine::BoxError, utils::abort::AbortSignal},
    kosong::contract::usage::TokenUsage,
};

use super::SessionSwarmTask;

pub const INITIAL_LAUNCH_LIMIT: usize = 5;
pub const INITIAL_LAUNCH_INTERVAL: Duration = Duration::from_millis(700);
pub const RATE_LIMIT_RETRY_BASE: Duration = Duration::from_secs(3);
pub const RATE_LIMIT_RETRY_FACTOR: u32 = 2;
pub const RATE_LIMIT_CAPACITY_SHRINK_INTERVAL: Duration = Duration::from_secs(2);
pub const RATE_LIMIT_CAPACITY_RECOVERY_INTERVAL: Duration = Duration::from_secs(3 * 60);
pub const RATE_LIMIT_SUSPENDED_REASON: &str = "Provider rate limit; subagent requeued for retry.";

/// Original: `AgentRunAttemptOptions`.
#[derive(Clone)]
pub struct AgentRunAttemptOptions {
    pub parent_tool_call_id: String,
    pub parent_tool_call_uuid: Option<String>,
    pub prompt: String,
    pub description: String,
    pub swarm_index: Option<u64>,
    pub run_in_background: bool,
    pub signal: AbortSignal,
    pub on_ready: Option<Arc<dyn Fn() + Send + Sync>>,
    pub suppress_rate_limit_failure_event: bool,
}

/// Original: `AgentSpawnAttemptOptions`.
#[derive(Clone)]
pub struct AgentSpawnAttemptOptions {
    pub profile_name: String,
    pub swarm_item: Option<String>,
    pub attempt: AgentRunAttemptOptions,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRunCompletion {
    pub result: String,
    pub usage: Option<TokenUsage>,
}

/// Original: `AgentRunAttemptHandle`.
pub struct AgentRunAttemptHandle {
    pub agent_id: String,
    pub profile_name: String,
    pub completion: BoxFuture<'static, Result<AgentRunCompletion, BoxError>>,
}

/// Original: `AgentRunSuspendedEvent`.
#[derive(Clone)]
pub struct AgentRunSuspendedEvent<T> {
    pub task: SessionSwarmTask<T>,
    pub agent_id: String,
    pub reason: String,
}

/// Launch boundary used by `AgentRunBatch`.
///
/// Spawn, resume, and retry stay separate because the session lifecycle
/// service gives each path different persistence and parent-child semantics.
#[async_trait]
pub trait AgentRunBatchLauncher<T>: Send + Sync {
    async fn spawn(
        &self,
        options: AgentSpawnAttemptOptions,
    ) -> Result<AgentRunAttemptHandle, BoxError>;
    async fn resume(
        &self,
        agent_id: &str,
        options: AgentRunAttemptOptions,
    ) -> Result<AgentRunAttemptHandle, BoxError>;
    async fn retry(
        &self,
        agent_id: &str,
        options: AgentRunAttemptOptions,
    ) -> Result<AgentRunAttemptHandle, BoxError>;
    fn suspended(&self, _event: AgentRunSuspendedEvent<T>) {}
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AgentRunBatchOptions {
    pub max_concurrency: Option<usize>,
}

/// Original: `retry.createTimeout(..., { randomize: false })` in
/// `requeueRateLimited()`. The first retry is three seconds and every later
/// retry doubles without a maximum cap.
pub fn rate_limit_retry_delay(retry_count: u32) -> Duration {
    let exponent = retry_count.saturating_sub(1);
    RATE_LIMIT_RETRY_BASE.saturating_mul(RATE_LIMIT_RETRY_FACTOR.saturating_pow(exponent))
}

pub const AGENT_SWARM_MAX_CONCURRENCY_ENV: &str = "KIMI_CODE_AGENT_SWARM_MAX_CONCURRENCY";

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{AGENT_SWARM_MAX_CONCURRENCY_ENV} must be a positive integer, got {raw:?}.")]
pub struct SwarmMaxConcurrencyError {
    pub raw: String,
}

pub fn resolve_swarm_max_concurrency(
    env: &HashMap<String, String>,
) -> Result<Option<usize>, SwarmMaxConcurrencyError> {
    let Some(raw) = env.get(AGENT_SWARM_MAX_CONCURRENCY_ENV) else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let value = raw
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| SwarmMaxConcurrencyError { raw: raw.clone() })?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_retries_double_from_the_source_base_delay() {
        assert_eq!(rate_limit_retry_delay(0), Duration::from_secs(3));
        assert_eq!(rate_limit_retry_delay(1), Duration::from_secs(3));
        assert_eq!(rate_limit_retry_delay(2), Duration::from_secs(6));
        assert_eq!(rate_limit_retry_delay(3), Duration::from_secs(12));
    }

    #[test]
    fn resolves_optional_positive_concurrency_and_preserves_invalid_error() {
        assert_eq!(
            resolve_swarm_max_concurrency(&HashMap::new()).unwrap(),
            None
        );
        let valid = HashMap::from([(AGENT_SWARM_MAX_CONCURRENCY_ENV.into(), " 12 ".into())]);
        assert_eq!(resolve_swarm_max_concurrency(&valid).unwrap(), Some(12));
        let invalid = HashMap::from([(AGENT_SWARM_MAX_CONCURRENCY_ENV.into(), "1.5".into())]);
        assert_eq!(
            resolve_swarm_max_concurrency(&invalid)
                .unwrap_err()
                .to_string(),
            "KIMI_CODE_AGENT_SWARM_MAX_CONCURRENCY must be a positive integer, got \"1.5\"."
        );
    }
}
