//! Plan-mode context injection.
//!
//! Original: `agent/plan/injection/planModeInjection.ts`.

use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;

use super::AgentPlanServiceContract;
use crate::{
    _base::di::lifecycle::{Disposable, DisposableHandle, DisposeResult},
    agent::{
        context_injector::{
            AgentContextInjectorServiceContract, ContextInjectionContent, ContextInjectionContext,
            ContextInjectionError, ContextInjectionProvider,
        },
        context_memory::AgentContextMemoryServiceContract,
    },
    kosong::contract::message::Role,
};

const PLAN_MODE_DEDUP_MIN_TURNS: usize = 2;
const PLAN_MODE_FULL_REFRESH_TURNS: usize = 5;
const PLAN_MODE_INJECTION_VARIANT: &str = "plan_mode";

const PLAN_MODE_EXIT_REMINDER: &str = include_str!("injection/plan-mode-exit-reminder.md");
const PLAN_MODE_FULL_REMINDER: &str = include_str!("injection/plan-mode-full-reminder.md");
const PLAN_MODE_INLINE_FULL_REMINDER: &str =
    include_str!("injection/plan-mode-inline-full-reminder.md");
const PLAN_MODE_INLINE_REENTRY_REMINDER: &str =
    include_str!("injection/plan-mode-inline-reentry-reminder.md");
const PLAN_MODE_INLINE_SPARSE_REMINDER: &str =
    include_str!("injection/plan-mode-inline-sparse-reminder.md");
const PLAN_MODE_REENTRY_REMINDER: &str = include_str!("injection/plan-mode-reentry-reminder.md");
const PLAN_MODE_SPARSE_REMINDER: &str = include_str!("injection/plan-mode-sparse-reminder.md");

pub struct PlanModeInjection {
    registration: DisposableHandle,
}

impl PlanModeInjection {
    // Original: PlanModeInjection.constructor().
    pub fn new(
        injector: Arc<dyn AgentContextInjectorServiceContract>,
        plan: Arc<dyn AgentPlanServiceContract>,
        context: Arc<dyn AgentContextMemoryServiceContract>,
    ) -> Self {
        let was_active = Arc::new(Mutex::new(false));
        let provider: ContextInjectionProvider = Arc::new(move |injection| {
            let plan = Arc::clone(&plan);
            let context = Arc::clone(&context);
            let was_active = Arc::clone(&was_active);
            Box::pin(async move { plan_mode_injection(injection, plan, context, was_active).await })
                as BoxFuture<
                    'static,
                    Result<Option<ContextInjectionContent>, ContextInjectionError>,
                >
        });
        Self {
            registration: injector.register(PLAN_MODE_INJECTION_VARIANT.into(), provider),
        }
    }
}

impl Disposable for PlanModeInjection {
    fn dispose(&self) -> DisposeResult {
        self.registration.dispose()
    }
}

async fn plan_mode_injection(
    injection: ContextInjectionContext,
    plan: Arc<dyn AgentPlanServiceContract>,
    context: Arc<dyn AgentContextMemoryServiceContract>,
    was_active: Arc<Mutex<bool>>,
) -> Result<Option<ContextInjectionContent>, ContextInjectionError> {
    let data = plan
        .status()
        .await
        .map_err(|error| -> ContextInjectionError { Box::new(error) })?;
    let Some(data) = data else {
        let was_active = std::mem::replace(&mut *was_active.lock().unwrap(), false);
        return Ok(
            was_active.then(|| ContextInjectionContent::Text(PLAN_MODE_EXIT_REMINDER.into()))
        );
    };
    let was_active_before = std::mem::replace(&mut *was_active.lock().unwrap(), true);
    if !was_active_before {
        return Ok(Some(ContextInjectionContent::Text(
            if data.content.trim().is_empty() {
                full_reminder(Some(&data.path))
            } else {
                reentry_reminder(Some(&data.path))
            },
        )));
    }
    let variant = plan_mode_reminder_variant(injection.last_injected_at, &context.get());
    Ok(match variant {
        Some(PlanModeReminderVariant::Full) => Some(ContextInjectionContent::Text(full_reminder(
            Some(&data.path),
        ))),
        Some(PlanModeReminderVariant::Sparse) => Some(ContextInjectionContent::Text(
            sparse_reminder(Some(&data.path)),
        )),
        None => None,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlanModeReminderVariant {
    Full,
    Sparse,
}

// Original: planModeReminderVariant().
fn plan_mode_reminder_variant(
    injected_at: Option<usize>,
    history: &[crate::agent::context_memory::ContextMessage],
) -> Option<PlanModeReminderVariant> {
    let Some(injected_at) = injected_at else {
        return Some(PlanModeReminderVariant::Full);
    };
    let mut assistant_turns_since = 0;
    for message in history.iter().skip(injected_at.saturating_add(1)) {
        match message.message.role {
            Role::Assistant => assistant_turns_since += 1,
            Role::User => return Some(PlanModeReminderVariant::Full),
            Role::System | Role::Tool => {}
        }
    }
    if assistant_turns_since >= PLAN_MODE_FULL_REFRESH_TURNS {
        Some(PlanModeReminderVariant::Full)
    } else if assistant_turns_since >= PLAN_MODE_DEDUP_MIN_TURNS {
        Some(PlanModeReminderVariant::Sparse)
    } else {
        None
    }
}

fn with_plan_file_footer(body: &str, plan_file_path: Option<&str>) -> String {
    match plan_file_path.filter(|path| !path.is_empty()) {
        Some(path) => format!("{body}\n\nPlan file: {path}"),
        None => body.into(),
    }
}

fn full_reminder(plan_file_path: Option<&str>) -> String {
    plan_file_path.filter(|path| !path.is_empty()).map_or_else(
        || PLAN_MODE_INLINE_FULL_REMINDER.into(),
        |_| with_plan_file_footer(PLAN_MODE_FULL_REMINDER, plan_file_path),
    )
}

fn sparse_reminder(plan_file_path: Option<&str>) -> String {
    plan_file_path.filter(|path| !path.is_empty()).map_or_else(
        || PLAN_MODE_INLINE_SPARSE_REMINDER.into(),
        |_| with_plan_file_footer(PLAN_MODE_SPARSE_REMINDER, plan_file_path),
    )
}

fn reentry_reminder(plan_file_path: Option<&str>) -> String {
    plan_file_path.filter(|path| !path.is_empty()).map_or_else(
        || PLAN_MODE_INLINE_REENTRY_REMINDER.into(),
        |_| with_plan_file_footer(PLAN_MODE_REENTRY_REMINDER, plan_file_path),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{agent::context_memory::ContextMessage, kosong::contract::message::Message};

    fn message(role: Role) -> ContextMessage {
        ContextMessage {
            message: Message::new(role, vec![], vec![]),
            id: None,
            provider_message_id: None,
            origin: None,
            is_error: None,
            note: None,
        }
    }

    #[test]
    fn selects_full_sparse_and_no_reminder_from_history() {
        assert_eq!(
            plan_mode_reminder_variant(None, &[]),
            Some(PlanModeReminderVariant::Full)
        );
        assert_eq!(
            plan_mode_reminder_variant(Some(0), &[message(Role::Assistant)]),
            None
        );
        assert_eq!(
            plan_mode_reminder_variant(
                Some(0),
                &[
                    message(Role::System),
                    message(Role::Assistant),
                    message(Role::Assistant)
                ],
            ),
            Some(PlanModeReminderVariant::Sparse)
        );
        assert_eq!(
            plan_mode_reminder_variant(Some(0), &[message(Role::System), message(Role::User)]),
            Some(PlanModeReminderVariant::Full)
        );
    }

    #[test]
    fn chooses_inline_or_path_backed_reminders() {
        assert_eq!(full_reminder(None), PLAN_MODE_INLINE_FULL_REMINDER);
        assert!(full_reminder(Some("/plans/one.md")).ends_with("Plan file: /plans/one.md"));
        assert_eq!(sparse_reminder(None), PLAN_MODE_INLINE_SPARSE_REMINDER);
        assert_eq!(reentry_reminder(None), PLAN_MODE_INLINE_REENTRY_REMINDER);
        assert!(with_plan_file_footer("body", Some("/plan.md")).ends_with("Plan file: /plan.md"));
    }
}
