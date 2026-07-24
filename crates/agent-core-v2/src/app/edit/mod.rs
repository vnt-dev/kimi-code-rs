//! File-editing domain.
//!
//! Original: `packages/agent-core-v2/src/app/edit`.

pub mod edit_service;
pub mod text_model;

pub use edit_service::{EditApplyInput, EditApplyResult, EditService};
pub use text_model::TextModel;
