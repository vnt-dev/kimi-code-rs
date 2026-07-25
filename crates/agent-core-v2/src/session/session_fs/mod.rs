//! Session filesystem protocol and services.

pub mod errors;
pub mod git_context;

pub use git_context::{collect_git_context, parse_project_name, sanitize_remote_url};
