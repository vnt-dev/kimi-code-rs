//! Telemetry contracts, privacy filtering, and transports.

pub mod console_appender;
pub mod contract;
pub mod core_version;
pub mod privacy;

pub use console_appender::{ConsoleAppender, ConsoleAppenderOptions};
pub use contract::*;
pub use core_version::resolve_core_version;
pub use privacy::{clean_telemetry_properties, clean_telemetry_string};
