use std::{fmt, sync::Arc};

use futures_util::future::{BoxFuture, ready};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::_base::di::instantiation::ServiceIdentifier;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Off => "off",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        })
    }
}

pub type LogContext = Map<String, Value>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogEntryError {
    pub message: String,
    pub stack: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    pub timestamp_ms: i64,
    pub level: LogLevel,
    pub message: String,
    pub context: Option<LogContext>,
    pub error: Option<LogEntryError>,
}

pub trait LogWriter: Send + Sync {
    fn write(&self, entry: LogEntry);

    fn flush(&self) -> BoxFuture<'_, std::io::Result<()>> {
        Box::pin(ready(Ok(())))
    }

    fn close(&self) -> BoxFuture<'_, std::io::Result<()>> {
        Box::pin(ready(Ok(())))
    }

    fn flush_sync(&self) -> std::io::Result<()> {
        Ok(())
    }
}

pub trait Logger: Send + Sync {
    fn error(&self, message: &str, payload: Option<Value>);
    fn warn(&self, message: &str, payload: Option<Value>);
    fn info(&self, message: &str, payload: Option<Value>);
    fn debug(&self, message: &str, payload: Option<Value>);
    fn child(&self, context: LogContext) -> Arc<dyn Logger>;
}

#[derive(Clone)]
pub struct LogServiceHandle(pub Arc<dyn Logger>);

pub const LOG_SERVICE_ID: ServiceIdentifier<LogServiceHandle> =
    ServiceIdentifier::new("logService");

pub fn level_enabled(level: LogLevel, configured: LogLevel) -> bool {
    let order = |level| match level {
        LogLevel::Off => 0,
        LogLevel::Error => 1,
        LogLevel::Warn => 2,
        LogLevel::Info => 3,
        LogLevel::Debug => 4,
    };
    !matches!(level, LogLevel::Off)
        && !matches!(configured, LogLevel::Off)
        && order(level) <= order(configured)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_filter_preserves_source_order() {
        assert!(level_enabled(LogLevel::Error, LogLevel::Info));
        assert!(level_enabled(LogLevel::Info, LogLevel::Info));
        assert!(!level_enabled(LogLevel::Debug, LogLevel::Info));
        assert!(!level_enabled(LogLevel::Error, LogLevel::Off));
        assert!(!level_enabled(LogLevel::Off, LogLevel::Debug));
    }
}
