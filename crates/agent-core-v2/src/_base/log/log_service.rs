use std::{
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use futures_util::future::BoxFuture;
use serde_json::{Map, Value};

use super::{
    contract::{
        LOG_SERVICE_ID, LogContext, LogEntry, LogEntryError, LogLevel, LogPayload, LogService,
        LogServiceHandle, LogWriter, Logger, level_enabled,
    },
    file_log::{FileLogWriter, RotatingFileWriterOptions},
    formatter::FormatOptions,
    log_config::{LOG_OPTIONS_ID, LoggingConfig},
};
use crate::_base::di::{
    descriptors::SyncDescriptor,
    instantiation::ServicesAccessorExt,
    lifecycle::{Disposable, DisposeError, DisposeResult},
    scope::{InstantiationType, LifecycleScope, register_scoped_service},
};

#[derive(Clone)]
pub struct BoundLogger {
    writer: Arc<dyn LogWriter>,
    level: Arc<RwLock<LogLevel>>,
    bound: LogContext,
}

impl BoundLogger {
    pub fn new(writer: Arc<dyn LogWriter>, level: LogLevel) -> Self {
        Self {
            writer,
            level: Arc::new(RwLock::new(level)),
            bound: Map::new(),
        }
    }

    fn with_parts(
        writer: Arc<dyn LogWriter>,
        level: Arc<RwLock<LogLevel>>,
        bound: LogContext,
    ) -> Self {
        Self {
            writer,
            level,
            bound,
        }
    }

    pub fn level(&self) -> LogLevel {
        *self.level.read().unwrap()
    }

    pub fn set_level(&self, level: LogLevel) {
        *self.level.write().unwrap() = level;
    }

    /// Creates another concrete bound logger. `Logger::child()` erases this
    /// type for callers, while scoped services need the concrete form to keep
    /// exposing mutable level and flush behavior.
    pub fn with_context(&self, context: LogContext) -> Self {
        let mut bound = self.bound.clone();
        bound.extend(context);
        Self::with_parts(Arc::clone(&self.writer), Arc::clone(&self.level), bound)
    }

    // Original: BoundLogger.emit(). Bound context overrides payload context.
    fn emit(&self, level: LogLevel, message: &str, payload: Option<LogPayload>) {
        if !level_enabled(level, self.level()) {
            return;
        }
        let (mut context, error) = extract_payload(payload);
        for (key, value) in &self.bound {
            context.insert(key.clone(), value.clone());
        }
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        self.writer.write(LogEntry {
            timestamp_ms,
            level,
            message: message.to_owned(),
            context: (!context.is_empty()).then_some(context),
            error,
        });
    }
}

impl Logger for BoundLogger {
    fn error(&self, message: &str, payload: Option<LogPayload>) {
        self.emit(LogLevel::Error, message, payload);
    }

    fn warn(&self, message: &str, payload: Option<LogPayload>) {
        self.emit(LogLevel::Warn, message, payload);
    }

    fn info(&self, message: &str, payload: Option<LogPayload>) {
        self.emit(LogLevel::Info, message, payload);
    }

    fn debug(&self, message: &str, payload: Option<LogPayload>) {
        self.emit(LogLevel::Debug, message, payload);
    }

    fn child(&self, context: LogContext) -> Arc<dyn Logger> {
        Arc::new(self.with_context(context))
    }
}

fn extract_payload(payload: Option<LogPayload>) -> (LogContext, Option<LogEntryError>) {
    match payload {
        None => (Map::new(), None),
        Some(LogPayload::Context(context)) => (context, None),
        Some(LogPayload::Error(error)) => (Map::new(), Some(error)),
        Some(LogPayload::Value(Value::Object(context))) => (context, None),
        Some(LogPayload::Value(value)) => {
            let reason = match value {
                Value::String(value) => value,
                value => serde_json::to_string(&value).unwrap_or_else(|_| value.to_string()),
            };
            (
                Map::from_iter([("reason".into(), Value::String(reason))]),
                None,
            )
        }
    }
}

pub struct AppLogService {
    logger: BoundLogger,
    sink: Arc<FileLogWriter>,
    disposed: AtomicBool,
}

impl AppLogService {
    pub fn new(options: &LoggingConfig) -> Self {
        let sink = Arc::new(FileLogWriter::new(
            RotatingFileWriterOptions {
                path: options.global_log_path.clone().into(),
                max_bytes: options.global_max_bytes,
                files: options.global_files,
            },
            FormatOptions::default(),
        ));
        let writer: Arc<dyn LogWriter> = sink.clone();
        Self {
            logger: BoundLogger::new(writer, options.level),
            sink,
            disposed: AtomicBool::new(false),
        }
    }
}

impl Logger for AppLogService {
    fn error(&self, message: &str, payload: Option<LogPayload>) {
        self.logger.error(message, payload);
    }
    fn warn(&self, message: &str, payload: Option<LogPayload>) {
        self.logger.warn(message, payload);
    }
    fn info(&self, message: &str, payload: Option<LogPayload>) {
        self.logger.info(message, payload);
    }
    fn debug(&self, message: &str, payload: Option<LogPayload>) {
        self.logger.debug(message, payload);
    }
    fn child(&self, context: LogContext) -> Arc<dyn Logger> {
        self.logger.child(context)
    }
}

impl LogService for AppLogService {
    fn level(&self) -> LogLevel {
        self.logger.level()
    }

    fn set_level(&self, level: LogLevel) {
        self.logger.set_level(level);
    }

    fn flush(&self) -> BoxFuture<'_, std::io::Result<()>> {
        self.sink.flush()
    }
}

impl Disposable for AppLogService {
    fn dispose(&self) -> DisposeResult {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.sink.flush_sync().map_err(DisposeError::single)?;
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let sink = Arc::clone(&self.sink);
            runtime.spawn(async move {
                let _ = sink.close().await;
            });
        }
        Ok(())
    }
}

pub fn register_log_service() {
    register_scoped_service(
        LifecycleScope::App,
        LOG_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let options = accessor.get(LOG_OPTIONS_ID)?;
            let service: Arc<dyn LogService> = Arc::new(AppLogService::new(&options));
            Ok(LogServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "log",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::log::file_log::MemoryLogWriter;

    #[test]
    fn bound_logger_filters_extracts_and_merges_context() {
        let memory = Arc::new(MemoryLogWriter::default());
        let writer: Arc<dyn LogWriter> = memory.clone();
        let logger = BoundLogger::new(writer, LogLevel::Info);
        let child = logger.child(Map::from_iter([
            ("scope".into(), Value::String("agent".into())),
            ("same".into(), Value::String("bound".into())),
        ]));
        child.debug("hidden", None);
        child.info(
            "ready",
            Some(LogPayload::Context(Map::from_iter([
                ("value".into(), Value::from(2)),
                ("same".into(), Value::String("payload".into())),
            ]))),
        );
        child.error(
            "failed",
            Some(LogPayload::Error(LogEntryError {
                message: "boom".into(),
                stack: Some("stack".into()),
            })),
        );
        let entries = memory.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].context.as_ref().unwrap()["same"], "bound");
        assert_eq!(entries[0].context.as_ref().unwrap()["value"], 2);
        assert_eq!(entries[1].error.as_ref().unwrap().message, "boom");
    }

    #[test]
    fn children_share_dynamic_level_state() {
        let memory = Arc::new(MemoryLogWriter::default());
        let writer: Arc<dyn LogWriter> = memory.clone();
        let logger = BoundLogger::new(writer, LogLevel::Warn);
        let child = logger.child(Map::new());
        child.info("hidden", None);
        logger.set_level(LogLevel::Debug);
        child.debug("visible", Some(LogPayload::Value(Value::Bool(true))));
        let entries = memory.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].context.as_ref().unwrap()["reason"], "true");
    }
}
