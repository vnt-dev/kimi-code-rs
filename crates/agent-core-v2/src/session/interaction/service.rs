//! Session interaction request implementation.
//!
//! Original: `session/interaction/interactionService.ts`.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::sync::{Mutex, oneshot};

use crate::_base::{
    di::{
        descriptors::SyncDescriptor,
        instantiation::ServiceIdentifier,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    event::{Emitter, Event},
};

use super::{
    Interaction, InteractionKind, InteractionPendingChangedEvent, InteractionRequest,
    InteractionResolution,
};

pub const RECENTLY_RESOLVED_TTL_MS: i64 = 60_000;
pub const RECENTLY_RESOLVED_MAX: usize = 256;

struct Pending {
    interaction: Interaction,
    resolve: Option<oneshot::Sender<serde_json::Value>>,
}

#[derive(Default)]
struct State {
    pending: HashMap<String, Pending>,
    pending_order: VecDeque<String>,
    recently_resolved: HashMap<String, i64>,
    recently_resolved_order: VecDeque<String>,
    next_id: u64,
}

pub struct SessionInteractionService {
    state: Mutex<State>,
    changed: Arc<Emitter<InteractionPendingChangedEvent>>,
    resolved: Arc<Emitter<InteractionResolution>>,
}

#[derive(Clone)]
pub struct SessionInteractionServiceHandle(pub Arc<SessionInteractionService>);

impl std::ops::Deref for SessionInteractionServiceHandle {
    type Target = SessionInteractionService;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_INTERACTION_SERVICE_ID: ServiceIdentifier<SessionInteractionServiceHandle> =
    ServiceIdentifier::new("sessionInteractionService");

impl Default for SessionInteractionService {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionInteractionService {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            changed: Arc::new(Emitter::new()),
            resolved: Arc::new(Emitter::new()),
        }
    }

    pub fn on_did_change_pending(&self) -> Event<InteractionPendingChangedEvent> {
        self.changed.event()
    }
    pub fn on_did_resolve(&self) -> Event<InteractionResolution> {
        self.resolved.event()
    }

    // Original: request().
    pub async fn request(&self, request: InteractionRequest) -> serde_json::Value {
        let receiver = self.begin_request(request).await;
        receiver.await.unwrap_or(serde_json::Value::Null)
    }

    /// Parks a request and returns its single-use response receiver.
    ///
    /// This ownership split is required by Rust so typed facades can install
    /// cancellation behavior after the request has been atomically queued.
    /// `request()` remains the direct counterpart of the source method.
    pub(crate) async fn begin_request(
        &self,
        request: InteractionRequest,
    ) -> oneshot::Receiver<serde_json::Value> {
        let (sender, receiver) = oneshot::channel();
        self.park(request, Some(sender)).await;
        receiver
    }

    // Original: enqueue().
    pub async fn enqueue(&self, request: InteractionRequest) -> Interaction {
        self.park(request, None).await
    }

    // Original: respond().
    pub async fn respond(&self, id: &str, response: serde_json::Value) {
        let (pending, event) = {
            let mut state = self.state.lock().await;
            let Some(pending) = state.pending.remove(id) else {
                return;
            };
            state.pending_order.retain(|key| key != id);
            remember_resolved(&mut state, id);
            let event = pending_changed(&state);
            (pending, event)
        };
        if let Some(sender) = pending.resolve {
            let _ = sender.send(response.clone());
        }
        self.changed.fire(&event);
        self.resolved.fire(&InteractionResolution {
            id: id.into(),
            response,
        });
    }

    pub async fn list_pending(&self, kind: Option<InteractionKind>) -> Vec<Interaction> {
        let state = self.state.lock().await;
        state
            .pending_order
            .iter()
            .filter_map(|id| state.pending.get(id))
            .map(|pending| pending.interaction.clone())
            .filter(|interaction| kind.is_none_or(|kind| interaction.kind == kind))
            .collect()
    }

    pub async fn is_recently_resolved(&self, id: &str) -> bool {
        let mut state = self.state.lock().await;
        prune_resolved(&mut state);
        state.recently_resolved.contains_key(id)
    }

    // Original: cancelPendingForTurn().
    pub async fn cancel_pending_for_turn(&self, turn_id: crate::agent::TurnId) {
        let cancelled = {
            let mut state = self.state.lock().await;
            let ids = state
                .pending_order
                .iter()
                .filter(|id| {
                    state
                        .pending
                        .get(*id)
                        .and_then(|pending| pending.interaction.origin.turn_id)
                        == Some(turn_id)
                })
                .cloned()
                .collect::<Vec<_>>();
            let mut cancelled = Vec::new();
            for id in ids {
                if let Some(pending) = state.pending.remove(&id) {
                    state.pending_order.retain(|key| key != &id);
                    remember_resolved(&mut state, &id);
                    cancelled.push((id, pending));
                }
            }
            let event = (!cancelled.is_empty()).then(|| pending_changed(&state));
            (cancelled, event)
        };
        for (id, pending) in cancelled.0 {
            let response = serde_json::json!({"cancelled": true, "reason": "turn_ended"});
            if let Some(sender) = pending.resolve {
                let _ = sender.send(response.clone());
            }
            self.resolved.fire(&InteractionResolution { id, response });
        }
        if let Some(event) = cancelled.1 {
            self.changed.fire(&event);
        }
    }

    async fn park(
        &self,
        request: InteractionRequest,
        resolve: Option<oneshot::Sender<serde_json::Value>>,
    ) -> Interaction {
        let (interaction, event) = {
            let mut state = self.state.lock().await;
            let id = request.id.unwrap_or_else(|| {
                let id = format!("interaction-{}", state.next_id);
                state.next_id += 1;
                id
            });
            let interaction = Interaction {
                id: id.clone(),
                kind: request.kind,
                payload: request.payload,
                origin: request.origin.unwrap_or_default(),
                created_at: now_ms(),
            };
            state.pending_order.push_back(id.clone());
            state.pending.insert(
                id,
                Pending {
                    interaction: interaction.clone(),
                    resolve,
                },
            );
            (interaction, pending_changed(&state))
        };
        self.changed.fire(&event);
        interaction
    }
}

pub fn register_session_interaction_service() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_INTERACTION_SERVICE_ID,
        SyncDescriptor::new(|_| {
            Ok(SessionInteractionServiceHandle(Arc::new(
                SessionInteractionService::new(),
            )))
        }),
        InstantiationType::Eager,
        "interaction",
    );
}

fn pending_changed(state: &State) -> InteractionPendingChangedEvent {
    InteractionPendingChangedEvent {
        pending: state.pending_order.iter().cloned().collect(),
    }
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
fn prune_resolved(state: &mut State) {
    let now = now_ms();
    while let Some(id) = state.recently_resolved_order.front().cloned() {
        if state
            .recently_resolved
            .get(&id)
            .is_some_and(|at| now - at <= RECENTLY_RESOLVED_TTL_MS)
        {
            break;
        }
        state.recently_resolved_order.pop_front();
        state.recently_resolved.remove(&id);
    }
}
fn remember_resolved(state: &mut State, id: &str) {
    prune_resolved(state);
    while state.recently_resolved_order.len() >= RECENTLY_RESOLVED_MAX {
        if let Some(oldest) = state.recently_resolved_order.pop_front() {
            state.recently_resolved.remove(&oldest);
        }
    }
    state.recently_resolved.insert(id.into(), now_ms());
    state.recently_resolved_order.push_back(id.into());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::interaction::InteractionOrigin;
    #[tokio::test]
    async fn responds_and_cancels_pending_turn_requests_in_order() {
        let service = Arc::new(SessionInteractionService::new());
        let wait = {
            let service = Arc::clone(&service);
            tokio::spawn(async move {
                service
                    .request(InteractionRequest {
                        id: Some("one".into()),
                        kind: InteractionKind::UserTool,
                        payload: serde_json::Value::Null,
                        origin: Some(InteractionOrigin {
                            agent_id: None,
                            turn_id: Some(crate::agent::TurnId::new(3)),
                        }),
                    })
                    .await
            })
        };
        tokio::task::yield_now().await;
        assert_eq!(service.list_pending(None).await.len(), 1);
        service
            .cancel_pending_for_turn(crate::agent::TurnId::new(3))
            .await;
        assert_eq!(
            wait.await.unwrap(),
            serde_json::json!({"cancelled": true, "reason": "turn_ended"})
        );
        assert!(service.is_recently_resolved("one").await);
    }
}
