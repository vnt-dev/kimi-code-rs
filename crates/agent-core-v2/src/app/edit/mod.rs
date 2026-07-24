//! File-editing domain.
//!
//! Original: `packages/agent-core-v2/src/app/edit`.

pub mod edit_service;
pub mod file_edit;
pub mod file_edit_service;
pub mod text_model;

pub use edit_service::{EditApplyInput, EditApplyResult, EditService};
pub use file_edit::{
    FILE_EDIT_SERVICE_ID, FileEditInput, FileEditResult, FileEditServiceContract,
    FileEditServiceHandle,
};
pub use file_edit_service::{FileEditService, register_file_edit_service};
pub use text_model::TextModel;
