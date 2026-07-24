//! Kimi Code application protocol server.
//!
//! This crate is the Rust counterpart of
//! `packages/kap-server/src/index.ts`. Protocol DTOs live in the sibling
//! `kimi-code-protocol` crate and are re-exported here where the TypeScript
//! package exposed them directly.

pub mod middleware;
pub mod security;
pub mod transport;

pub use kimi_code_protocol::{Envelope, err_envelope, ok_envelope};
pub use security::bind_classify::{BindClass, ClassifyOptions, classify};
