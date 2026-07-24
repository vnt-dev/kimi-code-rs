//! Session agent profile catalog domain.
//! Original: `packages/agent-core-v2/src/session/sessionAgentProfileCatalog`.

pub mod contract;
pub mod explicit_file_agent_source;

pub use contract::*;
pub use explicit_file_agent_source::{
    EXPLICIT_FILE_AGENT_SOURCE_ID, ExplicitFileAgentSource, ExplicitFileAgentSourceHandle,
    register_explicit_file_agent_source,
};
