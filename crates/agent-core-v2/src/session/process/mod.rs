//! Session-scoped child-process execution.

pub mod contract;
pub mod service;

pub use contract::{
    ProcessExecOptions, SESSION_PROCESS_RUNNER_SERVICE_ID, SessionProcess,
    SessionProcessRunnerContract, SessionProcessRunnerError, SessionProcessRunnerHandle,
    SessionProcessRunnerResult,
};
pub use service::{
    MissingProcessCommandError, SessionProcessRunner, register_session_process_runner,
};
