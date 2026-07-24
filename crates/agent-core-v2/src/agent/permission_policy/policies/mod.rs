//! Built-in agent permission policies.
//!
//! Original: `agent/permissionPolicy/policies`.

pub mod auto_mode_approve;
pub mod auto_mode_ask_user_question_deny;
pub mod default_tool_approve;
pub mod deny_all;
pub mod fallback_ask;
pub mod git_control_path_access_ask;
pub mod git_cwd_write_approve;
pub mod path_utils;
pub mod sensitive_file_access_ask;
pub mod user_configured_rule;
pub mod yolo_mode_approve;

pub use auto_mode_approve::*;
pub use auto_mode_ask_user_question_deny::*;
pub use default_tool_approve::*;
pub use deny_all::*;
pub use fallback_ask::*;
pub use git_control_path_access_ask::*;
pub use git_cwd_write_approve::*;
pub use path_utils::*;
pub use sensitive_file_access_ask::*;
pub use user_configured_rule::*;
pub use yolo_mode_approve::*;
