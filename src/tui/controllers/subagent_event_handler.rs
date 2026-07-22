use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

use crate::{
    sdk::{
        events::{AgentEvent, Event},
        types::HookResultEvent,
    },
    tui::{
        components::messages::tool_call::{SubagentMetrics, SubagentTextKind, ToolCallComponent},
        constant::kimi_tui::MAIN_AGENT_ID,
        types::ToolResultBlockData,
        utils::{
            event_payload::{args_record, serialize_tool_result_output},
            hook_result_format::format_hook_result_plain,
        },
    },
};

pub type SharedToolCall = Arc<Mutex<ToolCallComponent>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentInfo {
    pub parent_tool_call_id: String,
    pub name: String,
    pub run_in_background: bool,
    pub swarm_index: Option<u64>,
}

pub trait ChildAgentEventHost {
    fn route_btw_event(&mut self, event: &Event) -> bool;

    /// Applies the event when the parent tool call has a swarm progress panel.
    /// Returns false when no such panel exists.
    fn route_swarm_event(
        &mut self,
        parent_tool_call_id: &str,
        child_agent_id: &str,
        event: &AgentEvent,
    ) -> bool;

    fn tool_component(&self, parent_tool_call_id: &str) -> Option<SharedToolCall>;
    fn request_render(&mut self);
}

/// Routes non-lifecycle events emitted by an interactive child agent into the
/// parent tool-call presentation.
///
/// Original:
///   apps/kimi-code/src/tui/controllers/subagent-event-handler.ts
///   SubAgentEventHandler.routeChildAgentEvent()
pub struct ChildAgentEventRouter<H> {
    host: H,
    subagent_info: HashMap<String, SubagentInfo>,
}

impl<H> ChildAgentEventRouter<H>
where
    H: ChildAgentEventHost,
{
    pub fn new(host: H) -> Self {
        Self {
            host,
            subagent_info: HashMap::new(),
        }
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    pub fn subagent_info(&self, agent_id: &str) -> Option<&SubagentInfo> {
        self.subagent_info.get(agent_id)
    }

    pub fn remember_subagent(&mut self, agent_id: impl Into<String>, info: SubagentInfo) {
        self.subagent_info.insert(agent_id.into(), info);
    }

    pub fn clear(&mut self) {
        self.subagent_info.clear();
    }

    pub fn route_child_agent_event(&mut self, event: &Event) -> bool {
        if is_subagent_lifecycle_event(&event.event) || event.agent_id == MAIN_AGENT_ID {
            return false;
        }
        if self.host.route_btw_event(event) {
            return true;
        }

        let Some(info) = self.subagent_info.get(&event.agent_id).cloned() else {
            return true;
        };
        if info.parent_tool_call_id.is_empty() {
            return true;
        }
        if self
            .host
            .route_swarm_event(&info.parent_tool_call_id, &event.agent_id, &event.event)
        {
            self.host.request_render();
            return true;
        }

        let Some(component) = self.host.tool_component(&info.parent_tool_call_id) else {
            return true;
        };
        apply_event_to_tool_component(&mut lock_tool_component(&component), event, &info);
        true
    }
}

fn apply_event_to_tool_component(
    component: &mut ToolCallComponent,
    event: &Event,
    info: &SubagentInfo,
) {
    let child_agent_id = &event.agent_id;
    let state = component.state_mut();
    state.set_subagent_meta(child_agent_id, Some(&info.name));
    match &event.event {
        AgentEvent::HookResult {
            hook_event,
            content,
            blocked,
            ..
        } => state.append_subagent_text(
            &format_hook_result_plain(&HookResultEvent {
                hook_event: hook_event.clone(),
                content: content.clone(),
                blocked: blocked.unwrap_or(false),
            }),
            SubagentTextKind::Text,
        ),
        AgentEvent::AssistantDelta { delta, .. } => {
            state.append_subagent_text(delta, SubagentTextKind::Text);
        }
        AgentEvent::ThinkingDelta { delta, .. } => {
            state.append_subagent_text(delta, SubagentTextKind::Thinking);
        }
        AgentEvent::ToolCallStarted {
            tool_call_id,
            name,
            args,
            ..
        } => state.append_sub_tool_call(
            &child_tool_call_id(child_agent_id, tool_call_id),
            name,
            args_record(args),
        ),
        AgentEvent::ToolCallDelta {
            tool_call_id,
            name,
            arguments_part,
            ..
        } => state.append_sub_tool_call_delta(
            &child_tool_call_id(child_agent_id, tool_call_id),
            name.as_deref(),
            arguments_part.as_deref(),
        ),
        AgentEvent::ToolProgress {
            tool_call_id,
            update,
            ..
        } if matches!(
            update.kind,
            crate::sdk::types::ToolUpdateKind::Stdout | crate::sdk::types::ToolUpdateKind::Stderr
        ) =>
        {
            if let Some(text) = update.text.as_deref() {
                state.append_sub_tool_live_output(
                    &child_tool_call_id(child_agent_id, tool_call_id),
                    text,
                );
            }
        }
        AgentEvent::ToolResult {
            tool_call_id,
            output,
            is_error,
            ..
        } => {
            let output =
                serialize_tool_result_output(output).unwrap_or_else(|_| output.to_string());
            state.finish_sub_tool_call(ToolResultBlockData {
                tool_call_id: child_tool_call_id(child_agent_id, tool_call_id),
                output,
                is_error: *is_error,
                synthetic: None,
            });
        }
        AgentEvent::AgentStatusUpdated {
            context_tokens,
            usage,
            ..
        } => state.update_subagent_metrics(SubagentMetrics {
            context_tokens: *context_tokens,
            usage: usage
                .as_ref()
                .and_then(|usage| usage.total.clone().or_else(|| usage.current_turn.clone())),
        }),
        _ => {}
    }
}

fn child_tool_call_id(agent_id: &str, tool_call_id: &str) -> String {
    format!("{agent_id}:{tool_call_id}")
}

fn lock_tool_component(component: &SharedToolCall) -> MutexGuard<'_, ToolCallComponent> {
    match component.lock() {
        Ok(component) => component,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub fn is_subagent_lifecycle_event(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::SubagentSpawned { .. }
            | AgentEvent::SubagentStarted { .. }
            | AgentEvent::SubagentSuspended { .. }
            | AgentEvent::SubagentCompleted { .. }
            | AgentEvent::SubagentFailed { .. }
    )
}

pub fn is_user_cancelled_subagent_error(error: &str) -> bool {
    matches!(
        error.trim(),
        "Aborted by the user" | "The user manually interrupted this subagent batch."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::{
        sdk::{
            events::TurnEndReason,
            types::{SessionUsage, ToolUpdate, ToolUpdateKind},
        },
        tui::{components::messages::tool_call::SubagentPhase, types::ToolCallBlockData},
    };
    use serde_json::Map;

    #[derive(Default)]
    struct HostMock {
        btw_consumes: bool,
        swarm_consumes: bool,
        tool_components: HashMap<String, SharedToolCall>,
        renders: usize,
    }

    impl ChildAgentEventHost for HostMock {
        fn route_btw_event(&mut self, _: &Event) -> bool {
            self.btw_consumes
        }

        fn route_swarm_event(&mut self, _: &str, _: &str, _: &AgentEvent) -> bool {
            self.swarm_consumes
        }

        fn tool_component(&self, parent_tool_call_id: &str) -> Option<SharedToolCall> {
            self.tool_components.get(parent_tool_call_id).cloned()
        }

        fn request_render(&mut self) {
            self.renders += 1;
        }
    }

    fn event(agent_id: &str, event: AgentEvent) -> Event {
        Event {
            agent_id: agent_id.to_owned(),
            session_id: "session-1".to_owned(),
            event,
        }
    }

    fn parent_component() -> SharedToolCall {
        Arc::new(Mutex::new(ToolCallComponent::new(
            ToolCallBlockData {
                id: "parent".to_owned(),
                name: "Agent".to_owned(),
                args: Map::new(),
                description: None,
                streaming_arguments: None,
                streaming_started_at_ms: None,
                subagent: None,
                step: None,
                turn_id: None,
                truncated: None,
            },
            None,
            None,
        )))
    }

    fn configured_router() -> ChildAgentEventRouter<HostMock> {
        let mut host = HostMock::default();
        host.tool_components
            .insert("parent".to_owned(), parent_component());
        let mut router = ChildAgentEventRouter::new(host);
        router.remember_subagent(
            "child",
            SubagentInfo {
                parent_tool_call_id: "parent".to_owned(),
                name: "explorer".to_owned(),
                run_in_background: false,
                swarm_index: None,
            },
        );
        router
    }

    #[test]
    fn main_and_lifecycle_events_remain_for_the_session_handler() {
        let mut router = configured_router();
        assert!(!router.route_child_agent_event(&event(
            MAIN_AGENT_ID,
            AgentEvent::AssistantDelta {
                turn_id: 1,
                delta: "main".to_owned(),
            }
        )));
        assert!(!router.route_child_agent_event(&event(
            "child",
            AgentEvent::SubagentStarted {
                subagent_id: "child".to_owned(),
            }
        )));
    }

    #[test]
    fn btw_and_swarm_routing_have_priority_over_tool_component_updates() {
        let mut router = configured_router();
        router.host_mut().btw_consumes = true;
        assert!(router.route_child_agent_event(&event(
            "child",
            AgentEvent::AssistantDelta {
                turn_id: 1,
                delta: "btw".to_owned(),
            }
        )));
        router.host_mut().btw_consumes = false;
        router.host_mut().swarm_consumes = true;
        assert!(router.route_child_agent_event(&event(
            "child",
            AgentEvent::AssistantDelta {
                turn_id: 1,
                delta: "swarm".to_owned(),
            }
        )));
        assert_eq!(router.host().renders, 1);
    }

    #[test]
    fn streams_child_text_tools_results_and_metrics_into_parent_component() {
        let mut router = configured_router();
        for child_event in [
            AgentEvent::ThinkingDelta {
                turn_id: 1,
                delta: "thinking".to_owned(),
            },
            AgentEvent::AssistantDelta {
                turn_id: 1,
                delta: "answer".to_owned(),
            },
            AgentEvent::ToolCallStarted {
                turn_id: 1,
                tool_call_id: "read-1".to_owned(),
                name: "Read".to_owned(),
                args: serde_json::json!({"path":"src/lib.rs"}),
                description: None,
                display: None,
            },
            AgentEvent::ToolProgress {
                turn_id: 1,
                tool_call_id: "read-1".to_owned(),
                update: ToolUpdate {
                    kind: ToolUpdateKind::Stdout,
                    text: Some("live".to_owned()),
                    percent: None,
                    custom_kind: None,
                    custom_data: None,
                },
            },
            AgentEvent::ToolResult {
                turn_id: 1,
                tool_call_id: "read-1".to_owned(),
                output: serde_json::json!({"ok":true}),
                is_error: Some(false),
                synthetic: None,
            },
            AgentEvent::AgentStatusUpdated {
                model: None,
                thinking_effort: None,
                context_tokens: Some(99),
                max_context_tokens: None,
                context_usage: None,
                plan_mode: None,
                swarm_mode: None,
                permission: None,
                usage: Some(SessionUsage {
                    by_model: None,
                    current_turn: None,
                    total: Some(crate::sdk::types::TokenUsage {
                        input_cache_read: 1,
                        input_cache_creation: 2,
                        input_other: 3,
                        output: 4,
                    }),
                }),
                phase: None,
            },
        ] {
            assert!(router.route_child_agent_event(&event("child", child_event)));
        }

        let component = router
            .host()
            .tool_components
            .get("parent")
            .expect("parent component");
        let snapshot = lock_tool_component(component).get_subagent_snapshot();
        assert_eq!(snapshot.agent_name.as_deref(), Some("explorer"));
        assert_eq!(snapshot.phase, Some(SubagentPhase::Running));
        assert_eq!(snapshot.tool_count, 1);
        assert_eq!(snapshot.tokens, 99);
    }

    #[test]
    fn unknown_children_are_consumed_and_cancellation_messages_match_exactly() {
        let mut router = configured_router();
        assert!(router.route_child_agent_event(&event(
            "unregistered",
            AgentEvent::TurnEnded {
                turn_id: 1,
                reason: TurnEndReason::Completed,
                error: None,
                duration_ms: None,
            }
        )));
        assert!(is_user_cancelled_subagent_error(" Aborted by the user "));
        assert!(is_user_cancelled_subagent_error(
            "The user manually interrupted this subagent batch."
        ));
        assert!(!is_user_cancelled_subagent_error("cancelled"));
    }
}
