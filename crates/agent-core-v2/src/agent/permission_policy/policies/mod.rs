//! Built-in agent permission policies.
//!
//! Original: `agent/permissionPolicy/policies`.

pub mod agent_swarm_exclusive_deny;
pub mod auto_mode_approve;
pub mod auto_mode_ask_user_question_deny;
pub mod default_tool_approve;
pub mod deny_all;
pub mod fallback_ask;
pub mod git_control_path_access_ask;
pub mod git_cwd_write_approve;
pub mod goal_start_review_ask;
pub mod path_utils;
pub mod plan_mode_guard_deny;
pub mod plan_mode_tool_approve;
pub mod sensitive_file_access_ask;
pub mod session_approval_history;
pub mod user_configured_rule;
pub mod yolo_mode_approve;

pub use agent_swarm_exclusive_deny::*;
pub use auto_mode_approve::*;
pub use auto_mode_ask_user_question_deny::*;
pub use default_tool_approve::*;
pub use deny_all::*;
pub use fallback_ask::*;
pub use git_control_path_access_ask::*;
pub use git_cwd_write_approve::*;
pub use goal_start_review_ask::*;
pub use path_utils::*;
pub use plan_mode_guard_deny::*;
pub use plan_mode_tool_approve::*;
pub use sensitive_file_access_ask::*;
pub use session_approval_history::*;
pub use user_configured_rule::*;
pub use yolo_mode_approve::*;
