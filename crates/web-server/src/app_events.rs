use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use kimi_code_agent_core_v2::_base::di::lifecycle::{DisposableHandle, to_disposable};
use serde::Serialize;
use serde_json::Value;

pub const DESKTOP_STATE_CHANGED_EVENT: &str = "desktop-state-changed";
pub const GOAL_MODE_CHANGED_EVENT: &str = "goal-mode-changed";

pub type ApplicationEventHandler = Arc<dyn Fn(&str, Value) + Send + Sync>;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DesktopStateChange {
    WorkspaceUpserted {
        #[serde(rename = "workspaceId")]
        workspace_id: String,
    },
    WorkspaceRemoved {
        #[serde(rename = "workspaceId")]
        workspace_id: String,
    },
    SessionCreated {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "workspaceRoot")]
        workspace_root: String,
    },
    SessionForked {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    SessionArchived {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    SessionRestored {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    SessionsDeleted {
        #[serde(rename = "sessionIds")]
        session_ids: Vec<String>,
    },
}

#[derive(Default)]
pub(crate) struct ApplicationEventBus {
    next_id: AtomicU64,
    handlers: Mutex<HashMap<u64, ApplicationEventHandler>>,
    goal_modes: Mutex<HashMap<String, bool>>,
}

impl ApplicationEventBus {
    pub(crate) fn subscribe(
        self: &Arc<Self>,
        handler: ApplicationEventHandler,
    ) -> DisposableHandle {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.handlers
            .lock()
            .expect("application event registry poisoned")
            .insert(id, handler);
        let events: Weak<Self> = Arc::downgrade(self);
        to_disposable(move || {
            if let Some(events) = events.upgrade()
                && let Ok(mut handlers) = events.handlers.lock()
            {
                handlers.remove(&id);
            }
        })
    }

    pub(crate) fn emit(&self, event: &str, payload: Value) {
        let handlers = self
            .handlers
            .lock()
            .map(|handlers| handlers.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for handler in handlers {
            handler(event, payload.clone());
        }
    }

    pub(crate) fn desktop_state_changed(&self, change: DesktopStateChange) {
        if let Ok(mut goal_modes) = self.goal_modes.lock() {
            match &change {
                DesktopStateChange::SessionArchived { session_id } => {
                    goal_modes.remove(session_id);
                }
                DesktopStateChange::SessionsDeleted { session_ids } => {
                    for session_id in session_ids {
                        goal_modes.remove(session_id);
                    }
                }
                _ => {}
            }
        }
        if let Ok(payload) = serde_json::to_value(change) {
            self.emit(DESKTOP_STATE_CHANGED_EVENT, payload);
        }
    }

    pub(crate) fn goal_mode(&self, session_id: &str) -> bool {
        self.goal_modes
            .lock()
            .ok()
            .and_then(|goal_modes| goal_modes.get(session_id).copied())
            .unwrap_or(false)
    }

    pub(crate) fn set_goal_mode(&self, session_id: String, enabled: bool) {
        let changed = self
            .goal_modes
            .lock()
            .map(|mut goal_modes| {
                let previous = goal_modes.get(&session_id).copied().unwrap_or(false);
                if enabled {
                    goal_modes.insert(session_id.clone(), true);
                } else {
                    goal_modes.remove(&session_id);
                }
                previous != enabled
            })
            .unwrap_or(false);
        if changed {
            self.emit(
                GOAL_MODE_CHANGED_EVENT,
                serde_json::json!({
                    "sessionId": session_id,
                    "enabled": enabled,
                }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscriptions_receive_events_until_disposed() {
        let events = Arc::new(ApplicationEventBus::default());
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_for_handler = Arc::clone(&received);
        let subscription = events.subscribe(Arc::new(move |event, payload| {
            received_for_handler
                .lock()
                .unwrap()
                .push((event.to_owned(), payload));
        }));

        events.desktop_state_changed(DesktopStateChange::SessionCreated {
            session_id: "session-1".into(),
            workspace_root: "/workspace".into(),
        });
        subscription.dispose().unwrap();
        events.desktop_state_changed(DesktopStateChange::SessionArchived {
            session_id: "session-1".into(),
        });

        let received = received.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].0, DESKTOP_STATE_CHANGED_EVENT);
        assert_eq!(received[0].1["kind"], "session_created");
        assert_eq!(received[0].1["sessionId"], "session-1");
    }

    #[test]
    fn goal_mode_is_shared_and_broadcast_only_when_it_changes() {
        let events = Arc::new(ApplicationEventBus::default());
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_for_handler = Arc::clone(&received);
        let _subscription = events.subscribe(Arc::new(move |event, payload| {
            if event == GOAL_MODE_CHANGED_EVENT {
                received_for_handler.lock().unwrap().push(payload);
            }
        }));

        assert!(!events.goal_mode("session-1"));
        events.set_goal_mode("session-1".into(), true);
        events.set_goal_mode("session-1".into(), true);
        assert!(events.goal_mode("session-1"));
        events.set_goal_mode("session-1".into(), false);

        let received = received.lock().unwrap();
        assert_eq!(received.len(), 2);
        assert_eq!(received[0]["enabled"], true);
        assert_eq!(received[1]["enabled"], false);
    }

    #[test]
    fn deleting_sessions_clears_goal_modes_and_serializes_all_ids() {
        let events = Arc::new(ApplicationEventBus::default());
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_for_handler = Arc::clone(&received);
        let _subscription = events.subscribe(Arc::new(move |event, payload| {
            if event == DESKTOP_STATE_CHANGED_EVENT {
                received_for_handler.lock().unwrap().push(payload);
            }
        }));
        events.set_goal_mode("session-1".into(), true);
        events.set_goal_mode("session-2".into(), true);

        events.desktop_state_changed(DesktopStateChange::SessionsDeleted {
            session_ids: vec!["session-1".into(), "session-2".into()],
        });

        assert!(!events.goal_mode("session-1"));
        assert!(!events.goal_mode("session-2"));
        let received = received.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0]["kind"], "sessions_deleted");
        assert_eq!(
            received[0]["sessionIds"],
            serde_json::json!(["session-1", "session-2"])
        );
    }
}
