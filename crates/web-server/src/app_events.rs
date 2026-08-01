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
}

#[derive(Default)]
pub(crate) struct ApplicationEventBus {
    next_id: AtomicU64,
    handlers: Mutex<HashMap<u64, ApplicationEventHandler>>,
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
        if let Ok(payload) = serde_json::to_value(change) {
            self.emit(DESKTOP_STATE_CHANGED_EVENT, payload);
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
}
