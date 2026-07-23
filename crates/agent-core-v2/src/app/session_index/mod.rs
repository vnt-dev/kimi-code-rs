//! Persisted-session read-model contract and implementation.

pub mod contract;

pub use contract::{
    CHILD_SESSION_KIND, CHILD_SESSION_KIND_KEY, PARENT_SESSION_ID_KEY, SESSION_INDEX_SERVICE_ID,
    SessionIndexContract, SessionIndexError, SessionIndexHandle, SessionIndexResult,
    SessionListQuery, SessionSummary,
};
