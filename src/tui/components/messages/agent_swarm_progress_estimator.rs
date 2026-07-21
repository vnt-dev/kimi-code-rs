use std::collections::{HashMap, HashSet};

const DEFAULT_RATE_WINDOW_MS: f64 = 45_000.0;
const DEFAULT_CATCHUP_TIME_MS: f64 = 1_500.0;
const DEFAULT_WORKLOAD_SPREAD_FACTOR: f64 = 1.5;
const DEFAULT_UNFINISHED_PROGRESS_CAP: f64 = 0.85;
const DEFAULT_MAX_BOOST_GAIN: f64 = 0.75;
const RATE_TOOL_CONFIDENCE_SCALE: f64 = 4.0;
const BOOST_TOOL_CONFIDENCE_SCALE: f64 = 3.0;
const MIN_RATE_FACTOR: f64 = 0.25;
const HALF_TICK: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSwarmProgressEstimatorPhase {
    Pending,
    Queued,
    Suspended,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AgentSwarmProgressEstimatorOptions {
    pub rate_window_ms: Option<f64>,
    pub catchup_time_ms: Option<f64>,
    pub max_catchup_ticks_per_second: Option<f64>,
    pub workload_spread_factor: Option<f64>,
    pub unfinished_progress_cap: Option<f64>,
    pub max_boost_gain: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentSwarmProgressEstimateInput {
    pub member_key: String,
    pub phase: AgentSwarmProgressEstimatorPhase,
    pub capacity_ticks: f64,
    pub now_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgentSwarmProgressEstimate {
    pub raw_ticks: usize,
    pub display_ticks: f64,
    pub estimated_total_tool_calls: Option<f64>,
    pub estimated_progress: Option<f64>,
    pub target_progress: Option<f64>,
    pub target_ticks: Option<f64>,
    pub boosted: bool,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordToolCallResult {
    pub accepted: bool,
    pub raw_ticks: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalKind {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Default)]
struct MemberProgressState {
    started_at_ms: Option<f64>,
    paused_at_ms: Option<f64>,
    paused_duration_ms: f64,
    terminal_at_ms: Option<f64>,
    terminal_kind: Option<TerminalKind>,
    raw_ticks: usize,
    seen_tool_call_ids: HashSet<String>,
    tool_call_active_times_ms: Vec<f64>,
    display_ticks: f64,
    last_estimate_at_ms: Option<f64>,
    last_target_ticks: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct CompletedSample {
    total_ms: f64,
    raw_ticks: usize,
}

#[derive(Debug, Clone, Copy)]
struct EstimatePrior {
    completed_count: usize,
    typical_total_ms: f64,
    typical_tool_calls: f64,
    typical_rate_per_ms: f64,
}

#[derive(Debug)]
pub struct AgentSwarmProgressEstimator {
    members: HashMap<String, MemberProgressState>,
    rate_window_ms: f64,
    catchup_time_ms: f64,
    max_catchup_ticks_per_second: Option<f64>,
    workload_spread_factor: f64,
    unfinished_progress_cap: f64,
    max_boost_gain: f64,
}

impl Default for AgentSwarmProgressEstimator {
    fn default() -> Self {
        Self::new(AgentSwarmProgressEstimatorOptions::default())
    }
}

impl AgentSwarmProgressEstimator {
    // Original: agent-swarm-progress-estimator.ts constructor()
    pub fn new(options: AgentSwarmProgressEstimatorOptions) -> Self {
        Self {
            members: HashMap::new(),
            rate_window_ms: positive_or_default(options.rate_window_ms, DEFAULT_RATE_WINDOW_MS),
            catchup_time_ms: positive_or_default(options.catchup_time_ms, DEFAULT_CATCHUP_TIME_MS),
            max_catchup_ticks_per_second: positive_or_none(options.max_catchup_ticks_per_second),
            workload_spread_factor: spread_factor_or_default(
                options.workload_spread_factor,
                DEFAULT_WORKLOAD_SPREAD_FACTOR,
            ),
            unfinished_progress_cap: clamp_positive_ratio(
                options.unfinished_progress_cap,
                DEFAULT_UNFINISHED_PROGRESS_CAP,
            ),
            max_boost_gain: clamp_positive_ratio(options.max_boost_gain, DEFAULT_MAX_BOOST_GAIN),
        }
    }

    pub fn ensure_member(&mut self, member_key: &str, _now_ms: f64) {
        self.members.entry(member_key.to_owned()).or_default();
    }

    pub fn remove_missing_members(&mut self, member_keys: &[String]) {
        let live = member_keys
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        self.members.retain(|key, _| live.contains(key.as_str()));
    }

    pub fn mark_started(&mut self, member_key: &str, now_ms: f64) {
        let state = self.members.entry(member_key.to_owned()).or_default();
        Self::start_work(state, now_ms);
        if state.raw_ticks == 0 {
            state.raw_ticks = 1;
            state.display_ticks = state.display_ticks.max(1.0);
        }
        state.terminal_at_ms = None;
        state.terminal_kind = None;
    }

    pub fn mark_queued(&mut self, member_key: &str, now_ms: f64) {
        let state = self.members.entry(member_key.to_owned()).or_default();
        if state.started_at_ms.is_none() || state.terminal_kind.is_some() {
            return;
        }
        state.paused_at_ms.get_or_insert(now_ms);
        state.last_estimate_at_ms = Some(now_ms);
        state.last_target_ticks = None;
    }

    pub fn record_tool_call(
        &mut self,
        member_key: &str,
        tool_call_id: &str,
        now_ms: f64,
    ) -> RecordToolCallResult {
        let state = self.members.entry(member_key.to_owned()).or_default();
        Self::start_work(state, now_ms);
        if state.seen_tool_call_ids.contains(tool_call_id) {
            return RecordToolCallResult {
                accepted: false,
                raw_ticks: state.raw_ticks,
            };
        }
        state.seen_tool_call_ids.insert(tool_call_id.to_owned());
        state
            .tool_call_active_times_ms
            .push(Self::active_elapsed_ms(state, now_ms));
        state.raw_ticks += 1;
        state.display_ticks = (state.display_ticks + 1.0).max(state.raw_ticks as f64);
        state.terminal_at_ms = None;
        state.terminal_kind = None;
        RecordToolCallResult {
            accepted: true,
            raw_ticks: state.raw_ticks,
        }
    }

    pub fn mark_completed(&mut self, member_key: &str, now_ms: f64) {
        self.mark_terminal(member_key, now_ms, TerminalKind::Completed);
    }

    pub fn mark_failed(&mut self, member_key: &str, now_ms: f64) {
        self.mark_terminal(member_key, now_ms, TerminalKind::Failed);
    }

    pub fn mark_cancelled(&mut self, member_key: &str, now_ms: f64) {
        self.mark_terminal(member_key, now_ms, TerminalKind::Cancelled);
    }

    // Original: AgentSwarmProgressEstimator.estimate()
    pub fn estimate(
        &mut self,
        input: &AgentSwarmProgressEstimateInput,
    ) -> AgentSwarmProgressEstimate {
        self.members.entry(input.member_key.clone()).or_default();
        let state = self
            .members
            .get(&input.member_key)
            .cloned()
            .unwrap_or_default();
        let capacity_ticks = input.capacity_ticks.max(1.0);
        let raw_ticks = state.raw_ticks;
        let previous_display_ticks = state.display_ticks.max(raw_ticks as f64);
        let prior = self.build_prior();
        let base = AgentSwarmProgressEstimate {
            raw_ticks,
            display_ticks: previous_display_ticks,
            estimated_total_tool_calls: None,
            estimated_progress: None,
            target_progress: None,
            target_ticks: None,
            boosted: false,
            confidence: None,
        };
        let Some(prior) = prior else {
            self.finish_unboosted_estimate(&input.member_key, input.now_ms, previous_display_ticks);
            return base;
        };
        if input.phase != AgentSwarmProgressEstimatorPhase::Running || raw_ticks == 0 {
            self.finish_unboosted_estimate(&input.member_key, input.now_ms, previous_display_ticks);
            return base;
        }

        let completed_confidence = self.completed_sample_confidence(prior.completed_count);
        let estimated_total_tool_calls =
            self.estimate_total_tool_calls(&state, prior, input.now_ms, completed_confidence);
        let estimated_progress = self
            .unfinished_progress_cap
            .min(raw_ticks as f64 / estimated_total_tool_calls);
        let raw_progress = 1.0_f64.min(raw_ticks as f64 / capacity_ticks);
        if estimated_progress <= raw_progress {
            self.finish_unboosted_estimate(&input.member_key, input.now_ms, previous_display_ticks);
            return AgentSwarmProgressEstimate {
                estimated_total_tool_calls: Some(estimated_total_tool_calls),
                estimated_progress: Some(estimated_progress),
                ..base
            };
        }

        let tool_confidence = confidence(raw_ticks as f64, BOOST_TOOL_CONFIDENCE_SCALE);
        let boost_confidence = completed_confidence * tool_confidence;
        let boost_gain = self.max_boost_gain * boost_confidence;
        let target_progress = raw_progress + boost_gain * (estimated_progress - raw_progress);
        let target_ticks = (raw_ticks as f64).max(target_progress * capacity_ticks);
        let display_ticks = self.catch_up_display_ticks(
            &state,
            previous_display_ticks,
            target_ticks,
            capacity_ticks,
            input.now_ms,
        );
        if let Some(state) = self.members.get_mut(&input.member_key) {
            state.display_ticks = display_ticks;
            state.last_estimate_at_ms = Some(input.now_ms);
            state.last_target_ticks = Some(target_ticks);
        }
        AgentSwarmProgressEstimate {
            raw_ticks,
            display_ticks,
            estimated_total_tool_calls: Some(estimated_total_tool_calls),
            estimated_progress: Some(estimated_progress),
            target_progress: Some(target_progress),
            target_ticks: Some(target_ticks),
            boosted: display_ticks > raw_ticks as f64,
            confidence: Some(boost_confidence),
        }
    }

    pub fn estimate_all(
        &mut self,
        inputs: &[AgentSwarmProgressEstimateInput],
    ) -> HashMap<String, AgentSwarmProgressEstimate> {
        inputs
            .iter()
            .map(|input| (input.member_key.clone(), self.estimate(input)))
            .collect()
    }

    pub fn has_pending_catchup(&self) -> bool {
        self.members.values().any(|state| {
            state
                .last_target_ticks
                .is_some_and(|target| target > state.display_ticks + 0.1)
        })
    }

    fn finish_unboosted_estimate(&mut self, member_key: &str, now_ms: f64, display_ticks: f64) {
        if let Some(state) = self.members.get_mut(member_key) {
            state.display_ticks = display_ticks;
            state.last_estimate_at_ms = Some(now_ms);
            state.last_target_ticks = None;
        }
    }

    fn mark_terminal(&mut self, member_key: &str, now_ms: f64, terminal_kind: TerminalKind) {
        let state = self.members.entry(member_key.to_owned()).or_default();
        Self::finish_paused_interval(state, now_ms);
        state.terminal_at_ms = Some(now_ms);
        state.terminal_kind = Some(terminal_kind);
        state.display_ticks = state.display_ticks.max(state.raw_ticks as f64);
        state.last_target_ticks = None;
    }

    fn start_work(state: &mut MemberProgressState, now_ms: f64) {
        let was_queued = state.started_at_ms.is_none() || state.paused_at_ms.is_some();
        state.started_at_ms.get_or_insert(now_ms);
        Self::finish_paused_interval(state, now_ms);
        if was_queued {
            state.last_estimate_at_ms = None;
            state.last_target_ticks = None;
        }
    }

    fn finish_paused_interval(state: &mut MemberProgressState, now_ms: f64) {
        let Some(paused_at_ms) = state.paused_at_ms.take() else {
            return;
        };
        state.paused_duration_ms += (now_ms - paused_at_ms).max(0.0);
    }

    fn active_elapsed_ms(state: &MemberProgressState, now_ms: f64) -> f64 {
        let Some(started_at_ms) = state.started_at_ms else {
            return 0.0;
        };
        let current_paused_ms = state
            .paused_at_ms
            .map_or(0.0, |paused_at| (now_ms - paused_at).max(0.0));
        (now_ms - started_at_ms - state.paused_duration_ms - current_paused_ms).max(0.0)
    }

    fn build_prior(&self) -> Option<EstimatePrior> {
        let samples = self.completed_samples();
        if samples.is_empty() {
            return None;
        }
        Some(EstimatePrior {
            completed_count: samples.len(),
            typical_total_ms: log_median(samples.iter().map(|sample| sample.total_ms)),
            typical_tool_calls: log_median(samples.iter().map(|sample| sample.raw_ticks as f64)),
            typical_rate_per_ms: log_median(
                samples
                    .iter()
                    .map(|sample| (sample.raw_ticks as f64 + HALF_TICK) / sample.total_ms),
            ),
        })
    }

    fn completed_samples(&self) -> Vec<CompletedSample> {
        self.members
            .values()
            .filter_map(|state| {
                if state.terminal_kind != Some(TerminalKind::Completed) || state.raw_ticks == 0 {
                    return None;
                }
                let terminal_at_ms = state.terminal_at_ms?;
                state.started_at_ms?;
                let total_ms = Self::active_elapsed_ms(state, terminal_at_ms);
                (total_ms > 0.0).then_some(CompletedSample {
                    total_ms,
                    raw_ticks: state.raw_ticks,
                })
            })
            .collect()
    }

    fn estimate_total_tool_calls(
        &self,
        state: &MemberProgressState,
        prior: EstimatePrior,
        now_ms: f64,
        completed_confidence: f64,
    ) -> f64 {
        let elapsed_ms = Self::active_elapsed_ms(state, now_ms);
        let local_rate_per_ms = self.estimate_local_rate_per_ms(state, elapsed_ms);
        let rate_weight = confidence(state.raw_ticks as f64, RATE_TOOL_CONFIDENCE_SCALE);
        let clamped_local_rate_per_ms =
            local_rate_per_ms.max(prior.typical_rate_per_ms * MIN_RATE_FACTOR);
        let rate_per_ms = geometric_interpolate(
            prior.typical_rate_per_ms,
            clamped_local_rate_per_ms,
            rate_weight,
        );
        let total_ms = prior
            .typical_total_ms
            .max(elapsed_ms / self.unfinished_progress_cap);
        let estimated_total_tool_calls = rate_per_ms * total_ms;
        let bounded = self.soft_bound_total_tool_calls(
            estimated_total_tool_calls,
            prior,
            completed_confidence,
        );
        bounded
            .max(state.raw_ticks as f64 / self.unfinished_progress_cap)
            .max(1.0)
    }

    fn soft_bound_total_tool_calls(
        &self,
        total_tool_calls: f64,
        prior: EstimatePrior,
        completed_confidence: f64,
    ) -> f64 {
        let lower_bound = prior.typical_tool_calls / self.workload_spread_factor;
        let upper_bound = prior.typical_tool_calls * self.workload_spread_factor;
        let bounded = total_tool_calls.clamp(lower_bound, upper_bound);
        if bounded == total_tool_calls {
            total_tool_calls
        } else {
            geometric_interpolate(total_tool_calls, bounded, completed_confidence)
        }
    }

    fn estimate_local_rate_per_ms(&self, state: &MemberProgressState, elapsed_ms: f64) -> f64 {
        if elapsed_ms <= 0.0 || state.tool_call_active_times_ms.is_empty() {
            return 0.0;
        }
        let decayed_tool_calls = state
            .tool_call_active_times_ms
            .iter()
            .map(|time_ms| (-(elapsed_ms - time_ms).max(0.0) / self.rate_window_ms).exp())
            .sum::<f64>();
        let decayed_elapsed_ms =
            self.rate_window_ms * (1.0 - (-elapsed_ms / self.rate_window_ms).exp());
        if decayed_elapsed_ms <= 0.0 {
            0.0
        } else {
            decayed_tool_calls / decayed_elapsed_ms
        }
    }

    fn catch_up_display_ticks(
        &self,
        state: &MemberProgressState,
        previous_display_ticks: f64,
        target_ticks: f64,
        capacity_ticks: f64,
        now_ms: f64,
    ) -> f64 {
        if target_ticks <= previous_display_ticks {
            return previous_display_ticks;
        }
        let last_estimate_at_ms = state.last_estimate_at_ms.unwrap_or(now_ms);
        let elapsed_ms = (now_ms - last_estimate_at_ms).max(0.0);
        if elapsed_ms <= 0.0 {
            return previous_display_ticks;
        }
        let alpha = 1.0 - (-elapsed_ms / self.catchup_time_ms).exp();
        let desired_delta = (target_ticks - previous_display_ticks) * alpha;
        let max_catchup_ticks_per_second = self
            .max_catchup_ticks_per_second
            .unwrap_or(capacity_ticks / 2.0);
        let max_delta = (max_catchup_ticks_per_second * (elapsed_ms / 1_000.0)).max(0.0);
        previous_display_ticks + desired_delta.min(max_delta)
    }

    fn completed_sample_confidence(&self, completed_count: usize) -> f64 {
        confidence(completed_count as f64, 1.0 + self.workload_spread_factor)
    }
}

fn positive_or_default(value: Option<f64>, fallback: f64) -> f64 {
    value
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(fallback)
}

fn positive_or_none(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value > 0.0)
}

fn spread_factor_or_default(value: Option<f64>, fallback: f64) -> f64 {
    value
        .filter(|value| value.is_finite() && *value > 1.0)
        .unwrap_or(fallback)
}

fn clamp_positive_ratio(value: Option<f64>, fallback: f64) -> f64 {
    positive_or_default(value, fallback).clamp(0.01, 0.99)
}

fn confidence(count: f64, scale: f64) -> f64 {
    1.0 - (-(count.max(0.0)) / scale).exp()
}

fn geometric_interpolate(low: f64, high: f64, weight: f64) -> f64 {
    let safe_low = low.max(f64::EPSILON);
    let safe_high = high.max(f64::EPSILON);
    ((1.0 - weight) * safe_low.ln() + weight * safe_high.ln()).exp()
}

fn log_median(values: impl Iterator<Item = f64>) -> f64 {
    let mut logarithms = values
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(f64::ln)
        .collect::<Vec<_>>();
    logarithms.sort_by(f64::total_cmp);
    if logarithms.is_empty() {
        return 1.0;
    }
    let middle = logarithms.len() / 2;
    if logarithms.len() % 2 == 1 {
        logarithms[middle].exp()
    } else {
        ((logarithms[middle - 1] + logarithms[middle]) / 2.0).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(
        member_key: &str,
        phase: AgentSwarmProgressEstimatorPhase,
        now_ms: f64,
    ) -> AgentSwarmProgressEstimateInput {
        AgentSwarmProgressEstimateInput {
            member_key: member_key.to_owned(),
            phase,
            capacity_ticks: 56.0,
            now_ms,
        }
    }

    fn completed_sample(estimator: &mut AgentSwarmProgressEstimator) {
        estimator.mark_started("001", 0.0);
        for index in 0..10 {
            estimator.record_tool_call(
                "001",
                &format!("done-{index}"),
                1_000.0 + index as f64 * 1_000.0,
            );
        }
        estimator.mark_completed("001", 40_000.0);
    }

    #[test]
    fn started_member_has_one_tick_before_tool_calls() {
        let mut estimator = AgentSwarmProgressEstimator::default();
        estimator.mark_started("001", 0.0);
        let estimate = estimator.estimate(&input(
            "001",
            AgentSwarmProgressEstimatorPhase::Running,
            1_000.0,
        ));
        assert_eq!(estimate.raw_ticks, 1);
        assert_eq!(estimate.display_ticks, 1.0);
    }

    #[test]
    fn keeps_raw_ticks_without_samples_and_deduplicates_calls() {
        let mut estimator = AgentSwarmProgressEstimator::default();
        estimator.mark_started("001", 0.0);
        assert_eq!(
            estimator.record_tool_call("001", "read", 1_000.0),
            RecordToolCallResult {
                accepted: true,
                raw_ticks: 2
            }
        );
        assert_eq!(
            estimator.record_tool_call("001", "read", 2_000.0),
            RecordToolCallResult {
                accepted: false,
                raw_ticks: 2
            }
        );
        let estimate = estimator.estimate(&input(
            "001",
            AgentSwarmProgressEstimatorPhase::Running,
            3_000.0,
        ));
        assert_eq!(estimate.raw_ticks, 2);
        assert_eq!(estimate.display_ticks, 2.0);
        assert_eq!(estimate.estimated_total_tool_calls, None);
        assert!(!estimate.boosted);
    }

    #[test]
    fn queued_wait_before_start_does_not_catch_up_progress() {
        let mut estimator = AgentSwarmProgressEstimator::new(AgentSwarmProgressEstimatorOptions {
            catchup_time_ms: Some(1_000.0),
            max_catchup_ticks_per_second: Some(100.0),
            ..AgentSwarmProgressEstimatorOptions::default()
        });
        completed_sample(&mut estimator);
        estimator.ensure_member("002", 0.0);
        estimator.estimate(&input("002", AgentSwarmProgressEstimatorPhase::Queued, 0.0));
        estimator.mark_started("002", 60_000.0);
        let estimate = estimator.estimate(&input(
            "002",
            AgentSwarmProgressEstimatorPhase::Running,
            60_000.0,
        ));
        assert_eq!(estimate.raw_ticks, 1);
        assert_eq!(estimate.display_ticks, 1.0);
        assert!(estimate.target_ticks.is_some_and(|target| target > 1.0));
        assert!(!estimate.boosted);
    }

    #[test]
    fn smoothly_catches_up_without_jumping_to_target() {
        let mut estimator = AgentSwarmProgressEstimator::new(AgentSwarmProgressEstimatorOptions {
            catchup_time_ms: Some(1_000.0),
            max_catchup_ticks_per_second: Some(100.0),
            ..AgentSwarmProgressEstimatorOptions::default()
        });
        completed_sample(&mut estimator);
        estimator.mark_started("002", 0.0);
        for index in 0..3 {
            estimator.record_tool_call(
                "002",
                &format!("running-{index}"),
                5_000.0 + index as f64 * 5_000.0,
            );
        }
        let first = estimator.estimate(&input(
            "002",
            AgentSwarmProgressEstimatorPhase::Running,
            20_000.0,
        ));
        assert_eq!(first.raw_ticks, 4);
        assert_eq!(first.display_ticks, 4.0);
        assert!(
            first
                .estimated_total_tool_calls
                .is_some_and(|total| total > 4.0)
        );
        assert!(first.target_ticks.is_some_and(|target| target > 4.0));
        assert!(estimator.has_pending_catchup());

        let second = estimator.estimate(&input(
            "002",
            AgentSwarmProgressEstimatorPhase::Running,
            21_000.0,
        ));
        assert!(second.display_ticks > 4.0);
        assert!(
            second
                .target_ticks
                .is_some_and(|target| second.display_ticks < target)
        );
        assert!(second.boosted);
    }
}
