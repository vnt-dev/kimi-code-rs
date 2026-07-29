//! Native RPC support.
//!
//! Original: `agent/rpc`.

mod attachments;
mod contract;
pub mod core_api;
pub mod prompt_metadata;
mod service;

pub(crate) use attachments::*;
pub use contract::*;
pub use core_api::*;
pub use prompt_metadata::*;
pub use service::*;
