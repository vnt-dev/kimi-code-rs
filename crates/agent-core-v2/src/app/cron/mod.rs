//! Cron scheduler foundations.
//!
//! Original: `packages/agent-core-v2/src/app/cron`.

pub mod clock;

pub use clock::{ClockSources, SYSTEM_CLOCKS, resolve_clock_sources};
