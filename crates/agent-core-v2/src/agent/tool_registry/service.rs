use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicU64, Ordering},
};

use indexmap::IndexMap;

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        lifecycle::to_disposable,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    tool::{ErasedExecutableTool, ToolInfo, ToolSource},
};

use super::contract::{
    AGENT_TOOL_REGISTRY_SERVICE_ID, AgentToolRegistryServiceContract,
    AgentToolRegistryServiceHandle, ToolReference, ToolRegistrationOptions,
};

struct ToolEntry {
    tool: Arc<dyn ErasedExecutableTool>,
    source: ToolSource,
    registration_id: u64,
}

#[derive(Default)]
struct RegistryState {
    tools: IndexMap<String, ToolEntry>,
}

pub struct AgentToolRegistryService {
    state: Arc<Mutex<RegistryState>>,
    next_registration_id: AtomicU64,
}

impl Default for AgentToolRegistryService {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(RegistryState::default())),
            next_registration_id: AtomicU64::new(1),
        }
    }
}

impl AgentToolRegistryService {
    pub fn new() -> Self {
        Self::default()
    }
}

impl AgentToolRegistryServiceContract for AgentToolRegistryService {
    // Original: toolRegistryService.ts, register(). Replacement removes the
    // old entry before insertion, and the old disposable cannot remove the new
    // registration with the same name.
    fn register(
        &self,
        tool: Arc<dyn ErasedExecutableTool>,
        options: ToolRegistrationOptions,
    ) -> crate::_base::di::lifecycle::DisposableHandle {
        let name = tool.tool().name.clone();
        let registration_id = self.next_registration_id.fetch_add(1, Ordering::Relaxed);
        let entry = ToolEntry {
            tool,
            source: options.source.unwrap_or(ToolSource::Builtin),
            registration_id,
        };
        let mut state = self.state.lock().unwrap();
        state.tools.shift_remove(&name);
        state.tools.insert(name.clone(), entry);
        drop(state);

        let weak = Arc::downgrade(&self.state);
        to_disposable(move || unregister_if_current(&weak, &name, registration_id))
    }

    // Original: toolRegistryService.ts, list().
    fn list(&self) -> Vec<ToolInfo> {
        let state = self.state.lock().unwrap();
        let mut tools = state
            .tools
            .values()
            .map(|entry| ToolInfo {
                name: entry.tool.tool().name.clone(),
                description: entry.tool.tool().description.clone(),
                parameters: Some(entry.tool.tool().parameters.clone()),
                source: entry.source,
                info: None,
            })
            .collect::<Vec<_>>();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        tools
    }

    // Original: toolRegistryService.ts, listReferences().
    fn list_references(&self) -> Vec<ToolReference> {
        let state = self.state.lock().unwrap();
        let mut tools = state
            .tools
            .iter()
            .map(|(name, entry)| ToolReference {
                name: name.clone(),
                source: entry.source,
            })
            .collect::<Vec<_>>();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        tools
    }

    // Original: toolRegistryService.ts, resolve().
    fn resolve(&self, name: &str) -> Option<Arc<dyn ErasedExecutableTool>> {
        self.state
            .lock()
            .unwrap()
            .tools
            .get(name)
            .map(|entry| Arc::clone(&entry.tool))
    }
}

fn unregister_if_current(state: &Weak<Mutex<RegistryState>>, name: &str, registration_id: u64) {
    let Some(state) = state.upgrade() else { return };
    let mut state = state.lock().unwrap();
    if state
        .tools
        .get(name)
        .is_some_and(|entry| entry.registration_id == registration_id)
    {
        state.tools.shift_remove(name);
    }
}

// Original: toolRegistryService.ts, Agent-scope eager service registration.
pub fn register_agent_tool_registry_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_TOOL_REGISTRY_SERVICE_ID,
        SyncDescriptor::new(|_accessor| {
            let service: Arc<dyn AgentToolRegistryServiceContract> =
                Arc::new(AgentToolRegistryService::new());
            Ok(AgentToolRegistryServiceHandle(service))
        }),
        InstantiationType::Eager,
        "toolRegistry",
    );
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::Value;

    use super::*;
    use crate::{
        kosong::contract::tool::Tool,
        tool::{ExecutableTool, ToolExecution},
    };

    struct TestTool {
        definition: Tool,
    }

    impl TestTool {
        fn new(name: &str, description: &str) -> Self {
            Self {
                definition: Tool {
                    name: name.into(),
                    description: description.into(),
                    parameters: serde_json::Map::new(),
                    deferred: None,
                },
            }
        }
    }

    #[async_trait]
    impl ExecutableTool for TestTool {
        type Input = Value;

        fn tool(&self) -> &Tool {
            &self.definition
        }

        async fn resolve_execution(&self, _input: Value) -> ToolExecution {
            ToolExecution::Error(crate::tool::ExecutableToolResult::success("unused"))
        }
    }

    #[test]
    fn register_lists_resolves_replaces_and_disposes_by_entry_identity() {
        let registry = AgentToolRegistryService::new();
        let zed = registry.register(
            Arc::new(TestTool::new("Zed", "first")),
            ToolRegistrationOptions::default(),
        );
        let old_read = registry.register(
            Arc::new(TestTool::new("Read", "old")),
            ToolRegistrationOptions {
                source: Some(ToolSource::User),
            },
        );
        let new_read = registry.register(
            Arc::new(TestTool::new("Read", "new")),
            ToolRegistrationOptions {
                source: Some(ToolSource::Mcp),
            },
        );

        assert_eq!(
            registry
                .list()
                .iter()
                .map(|tool| (tool.name.as_str(), tool.description.as_str(), tool.source))
                .collect::<Vec<_>>(),
            [
                ("Read", "new", ToolSource::Mcp),
                ("Zed", "first", ToolSource::Builtin)
            ]
        );
        assert_eq!(registry.list_references()[0].name, "Read");
        assert_eq!(registry.resolve("Read").unwrap().tool().description, "new");

        old_read.dispose().unwrap();
        assert!(registry.resolve("Read").is_some());
        new_read.dispose().unwrap();
        assert!(registry.resolve("Read").is_none());
        zed.dispose().unwrap();
        assert!(registry.list().is_empty());
    }
}
