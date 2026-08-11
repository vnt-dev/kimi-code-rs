//! Mutable state owned by the agent tool executor.
//!
//! Original: `toolExecutorService.ts`, `recordDupType()` and the three
//! `register*()` methods.

use std::sync::{Arc, Mutex, Weak};

use crate::_base::di::lifecycle::{DisposableHandle, to_disposable};

use super::{MissingToolDescriber, ToolCallDupType, ToolCallGuard, UnavailableToolDescriber};

#[derive(Default)]
struct State {
    missing_tool_describer: Option<MissingToolDescriber>,
    unavailable_tool_describer: Option<UnavailableToolDescriber>,
    tool_call_guard: Option<ToolCallGuard>,
    tool_call_dup_types: std::collections::HashMap<String, ToolCallDupType>,
    dup_type_turn_id: Option<crate::agent::TurnId>,
}

/// Shared mutable state for one agent-scoped executor service.
#[derive(Clone, Default)]
pub struct ToolExecutorState {
    inner: Arc<Mutex<State>>,
}

impl ToolExecutorState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Original: `AgentToolExecutorService.recordDupType()`.
    pub fn record_dup_type(&self, tool_call_id: String, dup_type: ToolCallDupType) {
        self.inner
            .lock()
            .unwrap()
            .tool_call_dup_types
            .insert(tool_call_id, dup_type);
    }

    /// Original: the `execute()` turn-boundary reset before preflight.
    pub fn begin_turn(&self, turn_id: crate::agent::TurnId) {
        let mut state = self.inner.lock().unwrap();
        if state.dup_type_turn_id != Some(turn_id) {
            state.dup_type_turn_id = Some(turn_id);
            state.tool_call_dup_types.clear();
        }
    }

    /// Original: `trackToolCall()` consumes an entry after reporting it.
    pub fn take_dup_type(&self, tool_call_id: &str) -> Option<ToolCallDupType> {
        self.inner
            .lock()
            .unwrap()
            .tool_call_dup_types
            .remove(tool_call_id)
    }

    pub fn tool_call_guard(&self) -> Option<ToolCallGuard> {
        self.inner.lock().unwrap().tool_call_guard.clone()
    }

    pub fn unavailable_tool_describer(&self) -> Option<UnavailableToolDescriber> {
        self.inner
            .lock()
            .unwrap()
            .unavailable_tool_describer
            .clone()
    }

    pub fn missing_tool_describer(&self) -> Option<MissingToolDescriber> {
        self.inner.lock().unwrap().missing_tool_describer.clone()
    }

    /// Original: `AgentToolExecutorService.registerToolCallGuard()`.
    pub fn register_tool_call_guard(&self, guard: ToolCallGuard) -> DisposableHandle {
        self.inner.lock().unwrap().tool_call_guard = Some(Arc::clone(&guard));
        let state = Arc::downgrade(&self.inner);
        to_disposable(move || clear_guard_if_current(&state, &guard))
    }

    /// Original: `AgentToolExecutorService.registerUnavailableToolDescriber()`.
    pub fn register_unavailable_tool_describer(
        &self,
        describer: UnavailableToolDescriber,
    ) -> DisposableHandle {
        self.inner.lock().unwrap().unavailable_tool_describer = Some(Arc::clone(&describer));
        let state = Arc::downgrade(&self.inner);
        to_disposable(move || clear_unavailable_describer_if_current(&state, &describer))
    }

    /// Original: `AgentToolExecutorService.registerMissingToolDescriber()`.
    pub fn register_missing_tool_describer(
        &self,
        describer: MissingToolDescriber,
    ) -> DisposableHandle {
        self.inner.lock().unwrap().missing_tool_describer = Some(Arc::clone(&describer));
        let state = Arc::downgrade(&self.inner);
        to_disposable(move || clear_missing_describer_if_current(&state, &describer))
    }
}

fn clear_guard_if_current(state: &Weak<Mutex<State>>, guard: &ToolCallGuard) {
    let Some(state) = state.upgrade() else { return };
    let mut state = state.lock().unwrap();
    if state
        .tool_call_guard
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, guard))
    {
        state.tool_call_guard = None;
    }
}

fn clear_unavailable_describer_if_current(
    state: &Weak<Mutex<State>>,
    describer: &UnavailableToolDescriber,
) {
    let Some(state) = state.upgrade() else { return };
    let mut state = state.lock().unwrap();
    if state
        .unavailable_tool_describer
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, describer))
    {
        state.unavailable_tool_describer = None;
    }
}

fn clear_missing_describer_if_current(
    state: &Weak<Mutex<State>>,
    describer: &MissingToolDescriber,
) {
    let Some(state) = state.upgrade() else { return };
    let mut state = state.lock().unwrap();
    if state
        .missing_tool_describer
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, describer))
    {
        state.missing_tool_describer = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrations_only_dispose_their_own_current_value() {
        let state = ToolExecutorState::new();
        let first: ToolCallGuard = Arc::new(|_| Some("first".into()));
        let second: ToolCallGuard = Arc::new(|_| Some("second".into()));
        let first_disposable = state.register_tool_call_guard(Arc::clone(&first));
        let second_disposable = state.register_tool_call_guard(Arc::clone(&second));

        first_disposable.dispose().unwrap();
        assert!(state.tool_call_guard().is_some());
        second_disposable.dispose().unwrap();
        assert!(state.tool_call_guard().is_none());
    }

    #[test]
    fn duplicate_types_are_consumed_and_reset_on_a_new_turn() {
        let state = ToolExecutorState::new();
        state.begin_turn(crate::agent::TurnId::new(2));
        state.record_dup_type("call-a".into(), ToolCallDupType::SameStep);
        assert_eq!(
            state.take_dup_type("call-a"),
            Some(ToolCallDupType::SameStep)
        );
        assert_eq!(state.take_dup_type("call-a"), None);

        state.record_dup_type("call-b".into(), ToolCallDupType::CrossStep);
        state.begin_turn(crate::agent::TurnId::new(3));
        assert_eq!(state.take_dup_type("call-b"), None);
    }

    #[test]
    fn describer_registration_is_reversible() {
        let state = ToolExecutorState::new();
        let unavailable: UnavailableToolDescriber =
            Arc::new(|name| Some(format!("{name} unavailable")));
        let missing: MissingToolDescriber = Arc::new(|name| Some(format!("{name} missing")));
        let unavailable_disposable = state.register_unavailable_tool_describer(unavailable);
        let missing_disposable = state.register_missing_tool_describer(missing);

        assert!(state.unavailable_tool_describer().is_some());
        assert!(state.missing_tool_describer().is_some());
        unavailable_disposable.dispose().unwrap();
        missing_disposable.dispose().unwrap();
        assert!(state.unavailable_tool_describer().is_none());
        assert!(state.missing_tool_describer().is_none());
    }
}
