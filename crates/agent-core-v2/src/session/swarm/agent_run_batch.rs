//! Session swarm batch scheduler contracts and deterministic scheduling helpers.
//!
//! Original: `session/swarm/agentRunBatch.ts`.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use futures_util::future::BoxFuture;

use crate::{
    _base::{
        lifecycle::lifecycle_machine::BoxError,
        utils::abort::{
            AbortController, AbortSignal, abortable, create_deadline_abort_signal,
            is_user_cancellation, link_abort_signal,
        },
    },
    kosong::contract::{
        errors::{ChatProviderError, is_provider_rate_limit_error},
        usage::TokenUsage,
    },
};

use super::{SessionSwarmRunResult, SessionSwarmRunState, SessionSwarmRunStatus, SessionSwarmTask};

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

struct TaskState<T> {
    task: SessionSwarmTask<T>,
    agent_id: Option<String>,
    retry_agent_id: Option<String>,
    retry_count: u32,
    retry_ready_at: Instant,
    started: bool,
}

enum AttemptEvent {
    Ready(usize),
    Launched {
        index: usize,
        agent_id: String,
    },
    Completed {
        index: usize,
        agent_id: String,
        completion: AgentRunCompletion,
    },
    RateLimited {
        index: usize,
        agent_id: String,
        error: String,
    },
    Failed {
        index: usize,
        agent_id: Option<String>,
        status: SessionSwarmRunStatus,
        state: SessionSwarmRunState,
        error: String,
    },
}

/// Original: `AgentRunBatch`.
///
/// This owns only scheduler state. Lifecycle-specific spawning, resuming, and
/// retry persistence remain behind `AgentRunBatchLauncher`.
pub struct AgentRunBatch<T, L> {
    launcher: Arc<L>,
    tasks: Vec<SessionSwarmTask<T>>,
    options: AgentRunBatchOptions,
}

impl<T, L> AgentRunBatch<T, L>
where
    T: Clone + Send + Sync + 'static,
    L: AgentRunBatchLauncher<T> + 'static,
{
    pub fn new(
        launcher: Arc<L>,
        tasks: Vec<SessionSwarmTask<T>>,
        options: AgentRunBatchOptions,
    ) -> Self {
        Self {
            launcher,
            tasks,
            options,
        }
    }

    /// Run the batch once. A non-user batch abort rejects, while a user
    /// cancellation resolves one aborted result per unfinished task.
    pub async fn run(self) -> Result<Vec<SessionSwarmRunResult<T>>, BoxError> {
        let batch_signal = self
            .tasks
            .iter()
            .find_map(|task| task.base().signal.clone());
        let batch_controller = AbortController::new();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut states = self
            .tasks
            .into_iter()
            .map(|task| TaskState {
                task,
                agent_id: None,
                retry_agent_id: None,
                retry_count: 0,
                retry_ready_at: Instant::now(),
                started: false,
            })
            .collect::<Vec<_>>();
        if states.is_empty() {
            return Ok(Vec::new());
        }
        let mut pending = (0..states.len()).collect::<VecDeque<_>>();
        let mut results = (0..states.len()).map(|_| None).collect::<Vec<_>>();
        let mut active = HashMap::<usize, AbortController>::new();
        let mut normal_launch_count = 0;
        let mut next_normal_launch_at = Instant::now();
        let mut rate_limit_mode = false;
        let mut started_success_count = 0_usize;
        let mut rate_limit_capacity = 1_usize;
        let mut global_retry_interval = RATE_LIMIT_RETRY_BASE;
        let mut next_rate_limit_launch_at = Instant::now();
        let mut last_rate_limit_at = None::<Instant>;
        let mut last_capacity_shrink_at = None::<Instant>;
        let mut last_capacity_recovery_at = None::<Instant>;

        loop {
            if let Some(signal) = &batch_signal
                && signal.aborted()
            {
                let Some(reason) = signal.reason() else {
                    continue;
                };
                batch_controller.abort(Some((*reason).clone()));
                abort_active(&active, (*reason).clone());
                if is_user_cancellation(reason.as_ref()) {
                    return Ok(user_cancelled_results(&states, &results));
                }
                return Err(Box::new((*reason).clone()));
            }

            if results.iter().all(Option::is_some) {
                return Ok(results.into_iter().flatten().collect());
            }

            let now = Instant::now();
            if rate_limit_mode {
                recover_rate_limit_capacity(
                    now,
                    &pending,
                    last_rate_limit_at,
                    &mut last_capacity_recovery_at,
                    &mut rate_limit_capacity,
                    &mut next_rate_limit_launch_at,
                );
                if active.len() < rate_limit_capacity
                    && now >= next_rate_limit_launch_at
                    && let Some(position) = pending
                        .iter()
                        .position(|index| states[*index].retry_ready_at <= now)
                    && let Some(index) = pending.remove(position)
                {
                    start_attempt(
                        index,
                        &states[index],
                        Arc::clone(&self.launcher),
                        batch_controller.signal(),
                        sender.clone(),
                        &mut active,
                    );
                    next_rate_limit_launch_at = now + global_retry_interval;
                    continue;
                }
            } else if active.len() < self.options.max_concurrency.unwrap_or(usize::MAX) {
                let can_launch =
                    normal_launch_count < INITIAL_LAUNCH_LIMIT || now >= next_normal_launch_at;
                if can_launch && let Some(index) = pending.pop_front() {
                    start_attempt(
                        index,
                        &states[index],
                        Arc::clone(&self.launcher),
                        batch_controller.signal(),
                        sender.clone(),
                        &mut active,
                    );
                    normal_launch_count += 1;
                    if normal_launch_count >= INITIAL_LAUNCH_LIMIT {
                        next_normal_launch_at = now + INITIAL_LAUNCH_INTERVAL;
                    }
                    continue;
                }
            }

            let wake_at = if rate_limit_mode {
                rate_limit_wake_at(
                    &pending,
                    &states,
                    next_rate_limit_launch_at,
                    active.len(),
                    rate_limit_capacity,
                    last_rate_limit_at,
                    last_capacity_recovery_at,
                )
            } else if !pending.is_empty()
                && active.len() < self.options.max_concurrency.unwrap_or(usize::MAX)
            {
                Some(next_normal_launch_at)
            } else {
                None
            };

            match (batch_signal.as_ref(), wake_at) {
                (Some(signal), Some(wake_at)) => {
                    tokio::select! {
                        Some(event) = receiver.recv() => {
                            handle_attempt_event(
                                event, &mut states, &mut pending, &mut results, &mut active,
                                &mut rate_limit_mode, &mut started_success_count,
                                &mut rate_limit_capacity, &mut global_retry_interval,
                                &mut next_rate_limit_launch_at, &mut last_rate_limit_at,
                                &mut last_capacity_shrink_at, self.launcher.as_ref(),
                            );
                        }
                        _ = signal.cancelled() => {}
                        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(wake_at)) => {}
                    }
                }
                (Some(signal), None) => {
                    tokio::select! {
                        Some(event) = receiver.recv() => {
                            handle_attempt_event(
                                event, &mut states, &mut pending, &mut results, &mut active,
                                &mut rate_limit_mode, &mut started_success_count,
                                &mut rate_limit_capacity, &mut global_retry_interval,
                                &mut next_rate_limit_launch_at, &mut last_rate_limit_at,
                                &mut last_capacity_shrink_at, self.launcher.as_ref(),
                            );
                        }
                        _ = signal.cancelled() => {}
                    }
                }
                (None, Some(wake_at)) => {
                    tokio::select! {
                        Some(event) = receiver.recv() => {
                            handle_attempt_event(
                                event, &mut states, &mut pending, &mut results, &mut active,
                                &mut rate_limit_mode, &mut started_success_count,
                                &mut rate_limit_capacity, &mut global_retry_interval,
                                &mut next_rate_limit_launch_at, &mut last_rate_limit_at,
                                &mut last_capacity_shrink_at, self.launcher.as_ref(),
                            );
                        }
                        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(wake_at)) => {}
                    }
                }
                (None, None) => match receiver.recv().await {
                    Some(event) => handle_attempt_event(
                        event,
                        &mut states,
                        &mut pending,
                        &mut results,
                        &mut active,
                        &mut rate_limit_mode,
                        &mut started_success_count,
                        &mut rate_limit_capacity,
                        &mut global_retry_interval,
                        &mut next_rate_limit_launch_at,
                        &mut last_rate_limit_at,
                        &mut last_capacity_shrink_at,
                        self.launcher.as_ref(),
                    ),
                    None => return Err("AgentRunBatch attempt channel closed".into()),
                },
            }
        }
    }
}

fn start_attempt<T, L>(
    index: usize,
    state: &TaskState<T>,
    launcher: Arc<L>,
    batch_signal: AbortSignal,
    sender: tokio::sync::mpsc::UnboundedSender<AttemptEvent>,
    active: &mut HashMap<usize, AbortController>,
) where
    T: Clone + Send + Sync + 'static,
    L: AgentRunBatchLauncher<T> + 'static,
{
    let controller = AbortController::new();
    active.insert(index, controller.clone());
    let task = state.task.clone();
    let retry_agent_id = state.retry_agent_id.clone();
    tokio::spawn(async move {
        let batch_link = link_abort_signal(&batch_signal, controller.clone());
        let task_link = task
            .base()
            .signal
            .as_ref()
            .map(|signal| link_abort_signal(signal, controller.clone()));
        let mut deadline = task
            .base()
            .timeout
            .map(|timeout| create_deadline_abort_signal(&controller.signal(), timeout));
        let signal = deadline
            .as_ref()
            .map(|deadline| deadline.signal().clone())
            .unwrap_or_else(|| controller.signal());
        let on_ready_sender = sender.clone();
        let on_ready = Arc::new(move || {
            let _ = on_ready_sender.send(AttemptEvent::Ready(index));
        });
        let options = attempt_options(&task, signal.clone(), on_ready);
        let launched = match &retry_agent_id {
            Some(agent_id) => abortable(launcher.retry(agent_id, options), &signal)
                .await
                .map_err(|error| Box::new((*error).clone()) as BoxError)
                .and_then(|result| result),
            None => match &task {
                SessionSwarmTask::Resume {
                    resume_agent_id, ..
                } => abortable(launcher.resume(resume_agent_id, options), &signal)
                    .await
                    .map_err(|error| Box::new((*error).clone()) as BoxError)
                    .and_then(|result| result),
                SessionSwarmTask::Spawn(base) => {
                    let spawn_options = AgentSpawnAttemptOptions {
                        profile_name: base.profile_name.clone(),
                        swarm_item: base.swarm_item.clone(),
                        attempt: options,
                    };
                    abortable(launcher.spawn(spawn_options), &signal)
                        .await
                        .map_err(|error| Box::new((*error).clone()) as BoxError)
                        .and_then(|result| result)
                }
            },
        };
        let handle = match launched {
            Ok(handle) => handle,
            Err(error) => {
                let _ = sender.send(failed_event(
                    index,
                    None,
                    &controller.signal(),
                    deadline
                        .as_ref()
                        .is_some_and(|deadline| deadline.timed_out()),
                    task.base().timeout.is_some(),
                    error.to_string(),
                ));
                return;
            }
        };
        let agent_id = handle.agent_id;
        let _ = sender.send(AttemptEvent::Launched {
            index,
            agent_id: agent_id.clone(),
        });
        let completion = abortable(handle.completion, &signal)
            .await
            .map_err(|error| Box::new((*error).clone()) as BoxError)
            .and_then(|result| {
                result.map_err(|error| {
                    Box::new(crate::session::subagent::SharedAgentRunError(error.into()))
                        as BoxError
                })
            });
        drop(deadline.take());
        drop(task_link);
        drop(batch_link);
        match completion {
            Ok(completion) => {
                let _ = sender.send(AttemptEvent::Completed {
                    index,
                    agent_id,
                    completion,
                });
            }
            Err(error) if is_rate_limit_error(error.as_ref()) => {
                let _ = sender.send(AttemptEvent::RateLimited {
                    index,
                    agent_id,
                    error: error.to_string(),
                });
            }
            Err(error) => {
                let _ = sender.send(failed_event(
                    index,
                    Some(agent_id),
                    &controller.signal(),
                    false,
                    task.base().timeout.is_some(),
                    error.to_string(),
                ));
            }
        }
    });
}

fn attempt_options<T>(
    task: &SessionSwarmTask<T>,
    signal: AbortSignal,
    on_ready: Arc<dyn Fn() + Send + Sync>,
) -> AgentRunAttemptOptions {
    let base = task.base();
    AgentRunAttemptOptions {
        parent_tool_call_id: base.parent_tool_call_id.clone(),
        parent_tool_call_uuid: base.parent_tool_call_uuid.clone(),
        prompt: base.prompt.clone(),
        description: base.description.clone(),
        swarm_index: base.swarm_index,
        run_in_background: base.run_in_background,
        signal,
        on_ready: Some(on_ready),
        suppress_rate_limit_failure_event: true,
    }
}

fn failed_event(
    index: usize,
    agent_id: Option<String>,
    signal: &AbortSignal,
    timed_out: bool,
    has_timeout: bool,
    error: String,
) -> AttemptEvent {
    let user_cancelled = signal
        .reason()
        .is_some_and(|reason| is_user_cancellation(reason.as_ref()));
    let status = if user_cancelled {
        SessionSwarmRunStatus::Aborted
    } else {
        SessionSwarmRunStatus::Failed
    };
    let message = if timed_out && has_timeout {
        "Subagent timed out.".into()
    } else if user_cancelled {
        "The user manually interrupted this subagent batch.".into()
    } else {
        error
    };
    AttemptEvent::Failed {
        index,
        state: if agent_id.is_some() {
            SessionSwarmRunState::Started
        } else {
            SessionSwarmRunState::NotStarted
        },
        agent_id,
        status,
        error: message,
    }
}

fn is_rate_limit_error(error: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    error
        .downcast_ref::<ChatProviderError>()
        .is_some_and(is_provider_rate_limit_error)
}

#[allow(clippy::too_many_arguments)]
fn handle_attempt_event<T, L>(
    event: AttemptEvent,
    states: &mut [TaskState<T>],
    pending: &mut VecDeque<usize>,
    results: &mut [Option<SessionSwarmRunResult<T>>],
    active: &mut HashMap<usize, AbortController>,
    rate_limit_mode: &mut bool,
    started_success_count: &mut usize,
    rate_limit_capacity: &mut usize,
    global_retry_interval: &mut Duration,
    next_rate_limit_launch_at: &mut Instant,
    last_rate_limit_at: &mut Option<Instant>,
    last_capacity_shrink_at: &mut Option<Instant>,
    launcher: &L,
) where
    T: Clone,
    L: AgentRunBatchLauncher<T>,
{
    match event {
        AttemptEvent::Ready(index) => {
            if active.contains_key(&index) && !states[index].started {
                states[index].started = true;
                if *rate_limit_mode {
                    *global_retry_interval = RATE_LIMIT_RETRY_BASE;
                    *next_rate_limit_launch_at = Instant::now() + *global_retry_interval;
                } else {
                    *started_success_count += 1;
                }
            }
        }
        AttemptEvent::Launched { index, agent_id } => states[index].agent_id = Some(agent_id),
        AttemptEvent::Completed {
            index,
            agent_id,
            completion,
        } => {
            active.remove(&index);
            results[index] = Some(SessionSwarmRunResult {
                task: states[index].task.clone(),
                agent_id: Some(agent_id),
                status: SessionSwarmRunStatus::Completed,
                state: None,
                result: Some(completion.result),
                usage: completion.usage,
                error: None,
            });
        }
        AttemptEvent::Failed {
            index,
            agent_id,
            status,
            state,
            error,
        } => {
            active.remove(&index);
            results[index] = Some(SessionSwarmRunResult {
                task: states[index].task.clone(),
                agent_id,
                status,
                state: Some(state),
                result: None,
                usage: None,
                error: Some(error),
            });
        }
        AttemptEvent::RateLimited {
            index,
            agent_id,
            error,
        } => {
            active.remove(&index);
            if results
                .iter()
                .enumerate()
                .all(|(result_index, result)| result_index == index || result.is_some())
            {
                results[index] = Some(SessionSwarmRunResult {
                    task: states[index].task.clone(),
                    agent_id: Some(agent_id),
                    status: SessionSwarmRunStatus::Failed,
                    state: Some(SessionSwarmRunState::Started),
                    result: None,
                    usage: None,
                    error: Some(error),
                });
                return;
            }
            let now = Instant::now();
            let state = &mut states[index];
            state.agent_id = Some(agent_id.clone());
            state.retry_agent_id = Some(agent_id.clone());
            state.retry_count += 1;
            let retry_delay = rate_limit_retry_delay(state.retry_count);
            state.retry_ready_at = now + retry_delay;
            launcher.suspended(AgentRunSuspendedEvent {
                task: state.task.clone(),
                agent_id,
                reason: RATE_LIMIT_SUSPENDED_REASON.into(),
            });
            pending.push_front(index);
            *last_rate_limit_at = Some(now);
            if !*rate_limit_mode {
                *rate_limit_mode = true;
                *rate_limit_capacity = (*started_success_count).max(1).saturating_sub(1).max(1);
                *next_rate_limit_launch_at = now + RATE_LIMIT_RETRY_BASE;
                *last_capacity_shrink_at = Some(now);
            } else if last_capacity_shrink_at
                .is_none_or(|last| now.duration_since(last) >= RATE_LIMIT_CAPACITY_SHRINK_INTERVAL)
            {
                *rate_limit_capacity = rate_limit_capacity.saturating_sub(1).max(1);
                *last_capacity_shrink_at = Some(now);
            }
            if !state.started {
                *global_retry_interval =
                    (*global_retry_interval).saturating_mul(2).max(retry_delay);
                *next_rate_limit_launch_at =
                    (*next_rate_limit_launch_at).max(now + *global_retry_interval);
            } else {
                *next_rate_limit_launch_at =
                    (*next_rate_limit_launch_at).max(now + RATE_LIMIT_RETRY_BASE);
            }
        }
    }
}

fn recover_rate_limit_capacity(
    now: Instant,
    pending: &VecDeque<usize>,
    last_rate_limit_at: Option<Instant>,
    last_capacity_recovery_at: &mut Option<Instant>,
    capacity: &mut usize,
    next_launch_at: &mut Instant,
) {
    let Some(last_rate_limit_at) = last_rate_limit_at else {
        return;
    };
    if pending.is_empty() {
        return;
    }
    let latest = last_capacity_recovery_at
        .map(|recovery| recovery.max(last_rate_limit_at))
        .unwrap_or(last_rate_limit_at);
    if now >= latest + RATE_LIMIT_CAPACITY_RECOVERY_INTERVAL {
        *capacity += 1;
        *last_capacity_recovery_at = Some(now);
        *next_launch_at = (*next_launch_at).min(now);
    }
}

fn rate_limit_wake_at<T>(
    pending: &VecDeque<usize>,
    states: &[TaskState<T>],
    next_launch_at: Instant,
    active_count: usize,
    capacity: usize,
    last_rate_limit_at: Option<Instant>,
    last_capacity_recovery_at: Option<Instant>,
) -> Option<Instant> {
    if pending.is_empty() {
        return None;
    }
    let recovery = last_rate_limit_at.map(|last| {
        last_capacity_recovery_at
            .map(|recovery| recovery.max(last))
            .unwrap_or(last)
            + RATE_LIMIT_CAPACITY_RECOVERY_INTERVAL
    });
    if active_count >= capacity {
        return recovery;
    }
    let ready = pending
        .iter()
        .map(|index| states[*index].retry_ready_at)
        .min()
        .unwrap_or(next_launch_at);
    Some(recovery.map_or(next_launch_at.max(ready), |recovery| {
        next_launch_at.max(ready).min(recovery)
    }))
}

fn abort_active(
    active: &HashMap<usize, AbortController>,
    reason: crate::_base::utils::abort::AbortError,
) {
    for controller in active.values() {
        controller.abort(Some(reason.clone()));
    }
}

fn user_cancelled_results<T: Clone>(
    states: &[TaskState<T>],
    results: &[Option<SessionSwarmRunResult<T>>],
) -> Vec<SessionSwarmRunResult<T>> {
    states
        .iter()
        .enumerate()
        .map(|(index, state)| {
            results[index].clone().unwrap_or_else(|| {
                let started = state.started || state.agent_id.is_some();
                SessionSwarmRunResult {
                    task: state.task.clone(),
                    agent_id: state.agent_id.clone(),
                    status: SessionSwarmRunStatus::Aborted,
                    state: Some(if started {
                        SessionSwarmRunState::Started
                    } else {
                        SessionSwarmRunState::NotStarted
                    }),
                    result: None,
                    usage: None,
                    error: Some(if started {
                        "The user manually interrupted this subagent batch before this subagent finished."
                            .into()
                    } else {
                        "The user manually interrupted this subagent batch before this subagent was started."
                            .into()
                    }),
                }
            })
        })
        .collect()
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

    struct NoopLauncher;

    #[async_trait]
    impl AgentRunBatchLauncher<String> for NoopLauncher {
        async fn spawn(
            &self,
            _options: AgentSpawnAttemptOptions,
        ) -> Result<AgentRunAttemptHandle, BoxError> {
            Err("launcher should not be invoked".into())
        }

        async fn resume(
            &self,
            _agent_id: &str,
            _options: AgentRunAttemptOptions,
        ) -> Result<AgentRunAttemptHandle, BoxError> {
            Err("launcher should not be invoked".into())
        }

        async fn retry(
            &self,
            _agent_id: &str,
            _options: AgentRunAttemptOptions,
        ) -> Result<AgentRunAttemptHandle, BoxError> {
            Err("launcher should not be invoked".into())
        }
    }

    #[tokio::test]
    async fn empty_batch_finishes_without_launching() {
        let batch = AgentRunBatch::new(
            Arc::new(NoopLauncher),
            Vec::<SessionSwarmTask<String>>::new(),
            AgentRunBatchOptions::default(),
        );
        assert!(batch.run().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn user_cancelled_batch_marks_unstarted_tasks_aborted() {
        let controller = AbortController::new();
        controller.abort(Some(crate::_base::utils::abort::user_cancellation_reason()));
        let task = SessionSwarmTask::Spawn(super::super::SessionSwarmTaskBase {
            data: "work".into(),
            profile_name: "coder".into(),
            parent_tool_call_id: "call".into(),
            parent_tool_call_uuid: None,
            prompt: "do work".into(),
            description: "work".into(),
            swarm_index: Some(1),
            swarm_item: None,
            run_in_background: false,
            timeout: None,
            signal: Some(controller.signal()),
        });
        let batch = AgentRunBatch::new(
            Arc::new(NoopLauncher),
            vec![task],
            AgentRunBatchOptions::default(),
        );
        let results = batch.run().await.unwrap();
        assert_eq!(results[0].status, SessionSwarmRunStatus::Aborted);
        assert_eq!(results[0].state, Some(SessionSwarmRunState::NotStarted));
        assert_eq!(
            results[0].error.as_deref(),
            Some(
                "The user manually interrupted this subagent batch before this subagent was started."
            )
        );
    }

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
