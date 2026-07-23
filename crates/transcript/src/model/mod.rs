//! Transcript domain model.
//!
//! Original modules: `packages/transcript/src/model/*`.

pub mod attachment;
pub mod frame;
pub mod ids;
pub mod interaction;
pub mod item;
pub mod meta;
pub mod task;
pub mod todo;
pub mod turn;

pub use attachment::*;
pub use frame::*;
pub use ids::*;
pub use interaction::*;
pub use item::*;
pub use meta::*;
pub use task::*;
pub use todo::*;
pub use turn::*;

/// A TypeScript `unknown` property that may be absent or explicitly `null`.
pub type OptionalJsonValue = Option<Option<serde_json::Value>>;
