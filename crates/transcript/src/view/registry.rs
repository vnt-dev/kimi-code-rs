//! Renderer registry keyed by tool, input origin, and marker.
//!
//! Original:
//!   `packages/transcript/src/view/registry.ts`

use indexmap::IndexMap;

use crate::model::{OptionalJsonValue, ToolCallFrame, TranscriptTask, TurnOrigin};

pub struct ToolViewContext<'a> {
    pub frame: &'a ToolCallFrame,
    pub task: Option<&'a TranscriptTask>,
}

pub struct InputViewContext<'a> {
    pub origin: &'a TurnOrigin,
    pub prompt: Option<&'a str>,
}

pub struct MarkerViewContext<'a> {
    pub marker: &'a str,
    pub payload: &'a OptionalJsonValue,
}

pub struct ViewRegistryOptions<C> {
    pub fallback_tool: Option<C>,
}

impl<C> Default for ViewRegistryOptions<C> {
    fn default() -> Self {
        Self {
            fallback_tool: None,
        }
    }
}

pub struct ViewRegistry<C> {
    tool_renderers: IndexMap<String, C>,
    input_renderers: IndexMap<String, C>,
    marker_renderers: IndexMap<String, C>,
    fallback_tool: Option<C>,
}

impl<C> Default for ViewRegistry<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> ViewRegistry<C> {
    pub fn new() -> Self {
        Self::with_options(ViewRegistryOptions::default())
    }

    pub fn with_options(options: ViewRegistryOptions<C>) -> Self {
        Self {
            tool_renderers: IndexMap::new(),
            input_renderers: IndexMap::new(),
            marker_renderers: IndexMap::new(),
            fallback_tool: options.fallback_tool,
        }
    }

    pub fn register_tool(&mut self, key: impl Into<String>, renderer: C) -> &mut Self {
        self.tool_renderers
            .insert(key.into().to_lowercase(), renderer);
        self
    }

    pub fn register_input(&mut self, origin_kind: impl Into<String>, renderer: C) -> &mut Self {
        self.input_renderers.insert(origin_kind.into(), renderer);
        self
    }

    pub fn register_marker(&mut self, marker: impl Into<String>, renderer: C) -> &mut Self {
        self.marker_renderers.insert(marker.into(), renderer);
        self
    }

    pub fn resolve_tool(&self, frame: &ToolCallFrame) -> Option<&C> {
        let key = frame.view.as_deref().unwrap_or(&frame.name).to_lowercase();
        self.tool_renderers
            .get(&key)
            .or(self.fallback_tool.as_ref())
    }

    pub fn resolve_input(&self, origin: &TurnOrigin) -> Option<&C> {
        self.input_renderers.get(origin.kind())
    }

    pub fn resolve_marker(&self, marker: &str) -> Option<&C> {
        self.marker_renderers.get(marker)
    }
}

#[cfg(test)]
mod tests {
    use crate::model::{FrameId, ToolFrameState};

    use super::*;

    fn tool(name: &str, view: Option<&str>) -> ToolCallFrame {
        ToolCallFrame {
            frame_id: FrameId::from("f"),
            tool_call_id: "call".to_owned(),
            name: name.to_owned(),
            view: view.map(str::to_owned),
            state: ToolFrameState::Running,
            input: None,
            output: None,
            display: None,
            error: None,
            task_id: None,
            approval_id: None,
            todo_id: None,
            agent_refs: None,
        }
    }

    #[test]
    fn dispatches_case_insensitive_tools_hints_inputs_markers_and_fallback() {
        let mut registry = ViewRegistry::with_options(ViewRegistryOptions {
            fallback_tool: Some("generic"),
        });
        registry
            .register_tool("read", "read")
            .register_tool("swarm", "swarm")
            .register_input("cron", "cron")
            .register_marker("goal", "goal");

        assert_eq!(registry.resolve_tool(&tool("Read", None)), Some(&"read"));
        assert_eq!(
            registry.resolve_tool(&tool("AgentSwarm", Some("swarm"))),
            Some(&"swarm")
        );
        assert_eq!(registry.resolve_tool(&tool("Bash", None)), Some(&"generic"));
        assert_eq!(
            registry.resolve_input(&TurnOrigin::Cron {
                task_id: None,
                payload: None
            }),
            Some(&"cron")
        );
        assert_eq!(
            registry.resolve_input(&TurnOrigin::User { payload: None }),
            None
        );
        assert_eq!(registry.resolve_marker("goal"), Some(&"goal"));
    }
}
