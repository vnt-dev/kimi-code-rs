//! Immutable per-session identity and persistence facts.

pub mod context;

pub use context::{
    SESSION_CONTEXT_ID, SessionContext, SessionContextInput, make_session_context,
    session_context_seed,
};
