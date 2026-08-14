//! Goal-status context reminders.
//!
//! Original: `packages/agent-core-v2/src/agent/goal/injection/goalInjection.ts`.

use std::{collections::HashMap, sync::Arc};

use serde_json::Value;

use crate::{
    _base::{
        di::lifecycle::{Disposable, DisposableHandle, DisposeResult},
        utils::{render_prompt::render_prompt, xml_escape::escape_xml_text},
    },
    agent::{
        context_injector::{
            AgentContextInjectorServiceContract, ContextInjectionContent, ContextInjectionProvider,
            ContextInjectionResult,
        },
        goal::{GoalSnapshot, GoalStatus},
    },
};

const GOAL_INJECTION_VARIANT: &str = "goal";
const GOAL_ACTIVE_REMINDER: &str = include_str!("goal-active-reminder.md");
const GOAL_BLOCKED_REMINDER: &str = include_str!("goal-blocked-reminder.md");
const GOAL_PAUSED_REMINDER: &str = include_str!("goal-paused-reminder.md");

const BUDGET_GUIDANCE_NEARING: &str = "Budget guidance: you are nearing a budget. Converge on the objective and avoid starting new discretionary work.";
const BUDGET_GUIDANCE_WITHIN: &str =
    "Budget guidance: you are within budget. Make steady, focused progress toward the objective.";

pub trait GoalReader: Send + Sync {
    fn get_goal(&self) -> Option<GoalSnapshot>;
}

// Original: goalInjection.ts, GoalInjection.
pub struct GoalInjection {
    registration: DisposableHandle,
}

impl GoalInjection {
    pub fn new(
        goal: Arc<dyn GoalReader>,
        injector: &dyn AgentContextInjectorServiceContract,
    ) -> Self {
        let provider: ContextInjectionProvider = Arc::new(move |context| {
            let reminder = context
                .is_new_turn
                .then(|| goal.get_goal())
                .flatten()
                .and_then(|goal| goal_reminder(&goal));
            Box::pin(async move {
                Ok(reminder.map(ContextInjectionContent::Text)) as ContextInjectionResult
            })
        });
        Self {
            registration: injector.register(GOAL_INJECTION_VARIANT.into(), provider),
        }
    }
}

impl Disposable for GoalInjection {
    fn dispose(&self) -> DisposeResult {
        self.registration.dispose()
    }
}

// Original: goalInjection.ts, reminder().
pub fn goal_reminder(goal: &GoalSnapshot) -> Option<String> {
    match goal.status {
        GoalStatus::Active => Some(build_goal_reminder(goal)),
        GoalStatus::Blocked => Some(build_blocked_note(goal)),
        GoalStatus::Paused => Some(build_paused_note(goal)),
        GoalStatus::Complete => None,
    }
}

fn build_blocked_note(goal: &GoalSnapshot) -> String {
    render_prompt(
        GOAL_BLOCKED_REMINDER,
        &HashMap::from([
            ("reason_suffix".into(), Value::String(reason_suffix(goal))),
            (
                "objective".into(),
                Value::String(escape_xml_text(&goal.objective)),
            ),
            (
                "completion_criterion_block".into(),
                Value::String(completion_criterion_block(goal)),
            ),
        ]),
    )
}

fn build_paused_note(goal: &GoalSnapshot) -> String {
    render_prompt(
        GOAL_PAUSED_REMINDER,
        &HashMap::from([
            ("reason_suffix".into(), Value::String(reason_suffix(goal))),
            (
                "objective".into(),
                Value::String(escape_xml_text(&goal.objective)),
            ),
            (
                "completion_criterion_block".into(),
                Value::String(completion_criterion_block(goal)),
            ),
        ]),
    )
}

fn build_goal_reminder(goal: &GoalSnapshot) -> String {
    let budgets = format_budgets(goal);
    let budgets_block = if budgets.is_empty() {
        String::new()
    } else {
        format!("Budgets: {budgets}.\n")
    };
    render_prompt(
        GOAL_ACTIVE_REMINDER,
        &HashMap::from([
            (
                "objective".into(),
                Value::String(escape_xml_text(&goal.objective)),
            ),
            (
                "completion_criterion_block".into(),
                Value::String(completion_criterion_block(goal)),
            ),
            (
                "status".into(),
                Value::String(goal_status_name(goal.status).into()),
            ),
            (
                "progress".into(),
                Value::String(format!(
                    "{} continuation turns, {} tokens, {} elapsed",
                    goal.turns_used,
                    goal.tokens_used,
                    format_elapsed(goal.wall_clock_ms),
                )),
            ),
            ("budgets_block".into(), Value::String(budgets_block)),
            (
                "budget_guidance".into(),
                Value::String(
                    if is_nearing_budget(goal) {
                        BUDGET_GUIDANCE_NEARING
                    } else {
                        BUDGET_GUIDANCE_WITHIN
                    }
                    .into(),
                ),
            ),
        ]),
    )
}

fn reason_suffix(goal: &GoalSnapshot) -> String {
    goal.terminal_reason
        .as_ref()
        .map_or_else(String::new, |reason| {
            format!(" ({})", escape_xml_text(reason))
        })
}

fn completion_criterion_block(goal: &GoalSnapshot) -> String {
    goal.completion_criterion
        .as_ref()
        .map_or_else(String::new, |criterion| {
            format!(
                "<untrusted_completion_criterion>\n{}\n</untrusted_completion_criterion>\n",
                escape_xml_text(criterion)
            )
        })
}

fn format_budgets(goal: &GoalSnapshot) -> String {
    let mut lines = Vec::new();
    if let Some(budget) = goal.budget.turn_budget {
        lines.push(format!(
            "turns {}/{} (remaining {})",
            goal.turns_used,
            budget,
            optional_number(goal.budget.remaining_turns),
        ));
    }
    if let Some(budget) = goal.budget.token_budget {
        lines.push(format!(
            "tokens {}/{} (remaining {})",
            goal.tokens_used,
            budget,
            optional_number(goal.budget.remaining_tokens),
        ));
    }
    if let Some(budget) = goal.budget.wall_clock_budget_ms {
        lines.push(format!(
            "time {}/{} (remaining {})",
            format_elapsed(goal.wall_clock_ms),
            format_elapsed(budget),
            format_elapsed(goal.budget.remaining_wall_clock_ms.unwrap_or(0)),
        ));
    }
    lines.join("; ")
}

fn is_nearing_budget(goal: &GoalSnapshot) -> bool {
    max_budget_fraction(goal) >= 0.75
}

fn max_budget_fraction(goal: &GoalSnapshot) -> f64 {
    let mut fractions = Vec::new();
    if let Some(budget) = goal.budget.turn_budget.filter(|budget| *budget > 0) {
        fractions.push(goal.turns_used as f64 / budget as f64);
    }
    if let Some(budget) = goal.budget.token_budget.filter(|budget| *budget > 0) {
        fractions.push(goal.tokens_used as f64 / budget as f64);
    }
    if let Some(budget) = goal
        .budget
        .wall_clock_budget_ms
        .filter(|budget| *budget > 0)
    {
        fractions.push(goal.wall_clock_ms as f64 / budget as f64);
    }
    fractions.into_iter().fold(0.0, f64::max)
}

fn format_elapsed(milliseconds: u64) -> String {
    let total_seconds = ((milliseconds + 500) / 1000) as i64;
    if total_seconds < 60 {
        return format!("{total_seconds}s");
    }
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m{seconds:02}s");
    }
    format!("{}h{:02}m", minutes / 60, minutes % 60)
}

fn optional_number(value: Option<u64>) -> String {
    value.map_or_else(|| "undefined".into(), |value| value.to_string())
}

fn goal_status_name(status: GoalStatus) -> &'static str {
    match status {
        GoalStatus::Active => "active",
        GoalStatus::Paused => "paused",
        GoalStatus::Blocked => "blocked",
        GoalStatus::Complete => "complete",
    }
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::{
        _base::di::lifecycle::to_disposable,
        agent::{
            context_injector::{ContextInjectionContext, ContextInjectionError},
            goal::{GoalBudgetReport, GoalStatus},
        },
    };

    fn snapshot(status: GoalStatus) -> GoalSnapshot {
        GoalSnapshot {
            goal_id: "goal-1".into(),
            objective: "keep <this> & that".into(),
            completion_criterion: Some("finish > verify".into()),
            status,
            turns_used: 3,
            tokens_used: 750,
            wall_clock_ms: 61_000,
            budget: GoalBudgetReport {
                token_budget: Some(1_000),
                turn_budget: Some(4),
                wall_clock_budget_ms: Some(120_000),
                remaining_tokens: Some(250),
                remaining_turns: Some(1),
                remaining_wall_clock_ms: Some(59_000),
                token_budget_reached: false,
                turn_budget_reached: false,
                wall_clock_budget_reached: false,
                over_budget: false,
            },
            terminal_reason: Some("need <input>".into()),
        }
    }

    #[test]
    fn reminders_render_status_templates_budgets_and_escaped_untrusted_text() {
        let active = goal_reminder(&snapshot(GoalStatus::Active)).unwrap();
        assert!(active.contains(
            "<untrusted_objective>\nkeep &lt;this&gt; &amp; that\n</untrusted_objective>"
        ));
        assert!(active.contains("3 continuation turns, 750 tokens, 1m01s elapsed"));
        assert!(active.contains("Budgets: turns 3/4 (remaining 1); tokens 750/1000 (remaining 250); time 1m01s/2m00s (remaining 59s)."));
        assert!(active.contains(BUDGET_GUIDANCE_NEARING));

        let blocked = goal_reminder(&snapshot(GoalStatus::Blocked)).unwrap();
        assert!(blocked.contains("currently blocked (need &lt;input&gt;)"));
        assert!(blocked.contains("finish &gt; verify"));
        assert_eq!(goal_reminder(&snapshot(GoalStatus::Complete)), None);
    }

    struct Reader(Option<GoalSnapshot>);
    impl GoalReader for Reader {
        fn get_goal(&self) -> Option<GoalSnapshot> {
            self.0.clone()
        }
    }

    #[derive(Default)]
    struct Injector {
        provider: Mutex<Option<ContextInjectionProvider>>,
        disposed: Arc<AtomicBool>,
    }

    #[async_trait]
    impl AgentContextInjectorServiceContract for Injector {
        fn register(&self, name: String, provider: ContextInjectionProvider) -> DisposableHandle {
            assert_eq!(name, GOAL_INJECTION_VARIANT);
            *self.provider.lock() = Some(provider);
            let disposed = Arc::clone(&self.disposed);
            to_disposable(move || disposed.store(true, Ordering::SeqCst))
        }
        async fn inject_after_compaction(&self) -> Result<(), ContextInjectionError> {
            Ok(())
        }
    }

    impl Disposable for Injector {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    #[tokio::test]
    async fn injection_only_emits_on_new_turn_and_disposes_its_registration() {
        let injector = Injector::default();
        let goal: Arc<dyn GoalReader> = Arc::new(Reader(Some(snapshot(GoalStatus::Paused))));
        let injection = GoalInjection::new(goal, &injector);
        let provider = injector.provider.lock().clone().unwrap();
        let base = ContextInjectionContext {
            injected_positions: vec![],
            last_injected_at: None,
            is_new_turn: false,
        };
        assert_eq!(provider(base).await.unwrap(), None);
        let content = provider(ContextInjectionContext {
            injected_positions: vec![],
            last_injected_at: None,
            is_new_turn: true,
        })
        .await
        .unwrap();
        assert!(
            matches!(content, Some(ContextInjectionContent::Text(text)) if text.contains("currently paused"))
        );
        injection.dispose().unwrap();
        assert!(injector.disposed.load(Ordering::SeqCst));
    }
}
