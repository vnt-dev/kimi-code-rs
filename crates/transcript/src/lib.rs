//! Rust migration of `@moonshot-ai/transcript`.
//!
//! Original package: `packages/transcript/src/index.ts`.

pub mod model;
pub mod ops;

mod serde_utils;

pub use model::*;
pub use ops::*;
