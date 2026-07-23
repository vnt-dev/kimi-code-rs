//! Module-level executable-tool contribution collector.
//!
//! Original: `packages/agent-core-v2/src/agent/toolRegistry/toolContribution.ts`.

use std::sync::{Arc, LazyLock, RwLock};

use crate::{
    _base::di::{errors::DiError, instantiation::ServicesAccessor},
    tool::{ErasedExecutableTool, ToolSource},
};

pub type ToolFactory = Arc<
    dyn Fn(&dyn ServicesAccessor) -> Result<Arc<dyn ErasedExecutableTool>, DiError> + Send + Sync,
>;
pub type ToolContributionCondition = Arc<dyn Fn(&dyn ServicesAccessor) -> bool + Send + Sync>;

#[derive(Clone, Default)]
pub struct ToolContributionOptions {
    pub source: Option<ToolSource>,
    pub when: Option<ToolContributionCondition>,
}

#[derive(Clone)]
pub struct ToolContribution {
    pub factory: ToolFactory,
    pub options: ToolContributionOptions,
}

static TOOL_CONTRIBUTIONS: LazyLock<RwLock<Vec<ToolContribution>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

// Original: toolContribution.ts, registerTool(). Rust factories absorb both
// constructor injection and the source staticArgs callback.
pub fn register_tool(factory: ToolFactory, options: ToolContributionOptions) {
    TOOL_CONTRIBUTIONS
        .write()
        .unwrap()
        .push(ToolContribution { factory, options });
}

// Original: toolContribution.ts, getToolContributions().
pub fn get_tool_contributions() -> Vec<ToolContribution> {
    TOOL_CONTRIBUTIONS.read().unwrap().clone()
}

// Original: toolContribution.ts, _clearToolContributionsForTests().
pub fn clear_tool_contributions_for_tests() {
    TOOL_CONTRIBUTIONS.write().unwrap().clear();
}
