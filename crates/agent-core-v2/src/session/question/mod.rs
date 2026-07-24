//! Session-scoped ask-user question facade.
//!
//! Original: `session/question/question.ts` and `questionService.ts`.

pub mod contract;
pub mod service;

pub use contract::*;
pub use service::*;
