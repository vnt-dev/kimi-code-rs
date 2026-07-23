//! Rust migration of `@moonshot-ai/transcript`.
//!
//! Original package: `packages/transcript/src/index.ts`.

pub mod model;

mod serde_utils;

pub use model::*;
