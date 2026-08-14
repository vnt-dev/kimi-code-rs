//! Session-shared todo service.
//!
//! Original: `packages/agent-core-v2/src/session/todo/sessionTodoService.ts`.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use parking_lot::Mutex;

use futures_util::FutureExt;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            errors::DiError,
            lifecycle::{Disposable, DisposableHandle, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, ScopeHandle, register_scoped_service},
        },
        event::{Emitter, Event},
    },
    agent::{
        context_injector::{
            AGENT_CONTEXT_INJECTOR_SERVICE_ID, ContextInjectionContent, ContextInjectionError,
            ContextInjectionProvider,
        },
        context_memory::AGENT_CONTEXT_MEMORY_SERVICE_ID,
        tool_policy::AGENT_TOOL_POLICY_SERVICE_ID,
    },
    session::agent_lifecycle::{
        AGENT_LIFECYCLE_SERVICE_ID, AgentLifecycleServiceHandle, MAIN_AGENT_ID,
    },
    tool::ToolSource,
    wire::contract::{WIRE_SERVICE_ID, WireServiceHandle},
};

use super::{
    SESSION_TODO_SERVICE_ID, SessionTodoError, SessionTodoServiceContract,
    SessionTodoServiceHandle, TODO_LIST_REMINDER_VARIANT, TODO_LIST_TOOL_NAME, TODO_MODEL,
    TodoItem, TodoListReminderInput, ensure_todo_ops_registered, todo_list_stale_reminder,
    todo_set,
};

pub struct SessionTodoService {
    lifecycle: AgentLifecycleServiceHandle,
    on_did_change: Arc<Emitter<Vec<TodoItem>>>,
    agent_bindings: Mutex<HashMap<String, Vec<DisposableHandle>>>,
    disposables: DisposableStore,
}

impl SessionTodoService {
    pub fn new(lifecycle: AgentLifecycleServiceHandle) -> Result<Arc<Self>, DiError> {
        ensure_todo_ops_registered();
        let service = Arc::new(Self {
            lifecycle,
            on_did_change: Arc::new(Emitter::new()),
            agent_bindings: Mutex::new(HashMap::new()),
            disposables: DisposableStore::new(),
        });
        service.install()?;
        Ok(service)
    }

    fn install(self: &Arc<Self>) -> Result<(), DiError> {
        let weak = Arc::downgrade(self);
        self.disposables
            .add(self.lifecycle.on_did_create().subscribe(move |handle| {
                if let Some(service) = weak.upgrade() {
                    let _ = service.bind_agent(handle.clone());
                }
            }));
        let weak = Arc::downgrade(self);
        self.disposables
            .add(self.lifecycle.on_did_dispose().subscribe(move |agent_id| {
                if let Some(service) = weak.upgrade() {
                    service.dispose_agent_bindings(agent_id);
                }
            }));
        for handle in self.lifecycle.list(None) {
            self.bind_agent(handle)?;
        }
        Ok(())
    }

    fn main_wire(&self) -> Option<WireServiceHandle> {
        self.lifecycle
            .get(MAIN_AGENT_ID)?
            .get(WIRE_SERVICE_ID)
            .ok()
            .map(|wire| (*wire).clone())
    }

    fn bind_agent(self: &Arc<Self>, handle: ScopeHandle) -> Result<(), DiError> {
        let injector = handle.get(AGENT_CONTEXT_INJECTOR_SERVICE_ID)?;
        let weak = Arc::downgrade(self);
        let reminder_handle = handle.clone();
        let provider: ContextInjectionProvider = Arc::new(move |_| {
            let weak = Weak::clone(&weak);
            let handle = reminder_handle.clone();
            async move {
                let Some(service) = weak.upgrade() else {
                    return Ok(None);
                };
                service
                    .stale_reminder(&handle)
                    .map(|reminder| reminder.map(ContextInjectionContent::Text))
            }
            .boxed()
        });
        let binding = injector.register(TODO_LIST_REMINDER_VARIANT.into(), provider);
        self.agent_bindings
            .lock()
            .entry(handle.id().to_owned())
            .or_default()
            .push(binding);
        Ok(())
    }

    fn stale_reminder(
        &self,
        handle: &ScopeHandle,
    ) -> Result<Option<String>, ContextInjectionError> {
        let memory = handle
            .get(AGENT_CONTEXT_MEMORY_SERVICE_ID)
            .map_err(|error| Box::new(error) as ContextInjectionError)?;
        let policy = handle
            .get(AGENT_TOOL_POLICY_SERVICE_ID)
            .map_err(|error| Box::new(error) as ContextInjectionError)?;
        let active = policy.is_tool_active(TODO_LIST_TOOL_NAME, ToolSource::Builtin)?;
        let history = memory.get();
        let todos = self.get_todos();
        Ok(todo_list_stale_reminder(TodoListReminderInput {
            active,
            history: &history,
            todos: &todos,
        }))
    }

    fn dispose_agent_bindings(&self, agent_id: &str) {
        let bindings = self.agent_bindings.lock().remove(agent_id);
        if let Some(bindings) = bindings {
            for binding in bindings {
                let _ = binding.dispose();
            }
        }
    }
}

impl SessionTodoServiceContract for SessionTodoService {
    fn get_todos(&self) -> Vec<TodoItem> {
        self.main_wire()
            .map(|wire| wire.get_model(&TODO_MODEL))
            .unwrap_or_default()
    }

    fn set_todos(&self, todos: &[TodoItem]) -> Result<(), SessionTodoError> {
        let Some(wire) = self.main_wire() else {
            return Ok(());
        };
        let next = todos.to_vec();
        wire.dispatch([todo_set(&next)?])?;
        let current = wire.get_model(&TODO_MODEL);
        self.on_did_change.fire(&current);
        Ok(())
    }

    fn on_did_change(&self) -> Event<Vec<TodoItem>> {
        self.on_did_change.event()
    }
}

impl Disposable for SessionTodoService {
    fn dispose(&self) -> DisposeResult {
        let agent_ids = self
            .agent_bindings
            .lock()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for agent_id in agent_ids {
            self.dispose_agent_bindings(&agent_id);
        }
        let result = self.disposables.dispose();
        let emitter_result = self.on_did_change.dispose();
        result.and(emitter_result)
    }
}

pub fn register_session_todo_service() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_TODO_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            use crate::_base::di::instantiation::ServicesAccessorExt;
            let lifecycle = accessor.get(AGENT_LIFECYCLE_SERVICE_ID)?;
            let service = SessionTodoService::new((*lifecycle).clone())?;
            let contract: Arc<dyn SessionTodoServiceContract> = service;
            Ok(SessionTodoServiceHandle(contract))
        })
        .disposable(),
        InstantiationType::Eager,
        "todo",
    );
}
