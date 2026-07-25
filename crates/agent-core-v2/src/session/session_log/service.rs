//! Session-scoped file logger.
//!
//! Original: `session/sessionLog/sessionLogService.ts`, `SessionLogService`.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use futures_util::future::BoxFuture;
use serde_json::{Map, Value};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposeError, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        log::{
            BoundLogger, FileLogWriter, FormatOptions, LOG_OPTIONS_ID, LOG_SERVICE_ID, LogContext,
            LogLevel, LogPayload, LogService, LogServiceHandle, LogWriter, Logger,
            RotatingFileWriterOptions, resolve_session_log_path,
        },
    },
    session::session_context::{SESSION_CONTEXT_ID, SessionContext},
};

pub struct SessionLogService {
    logger: BoundLogger,
    sink: Arc<FileLogWriter>,
    disposed: AtomicBool,
}

impl SessionLogService {
    pub fn new(options: &crate::_base::log::LoggingConfig, session: &SessionContext) -> Self {
        let sink = Arc::new(FileLogWriter::new(
            RotatingFileWriterOptions {
                path: resolve_session_log_path(&session.session_dir).into(),
                max_bytes: options.session_max_bytes,
                files: options.session_files,
            },
            FormatOptions {
                omit_context_keys: ["sessionId".into()].into(),
                ..FormatOptions::default()
            },
        ));
        let writer: Arc<dyn LogWriter> = sink.clone();
        Self {
            logger: BoundLogger::new(writer, options.level).with_context(Map::from_iter([(
                "sessionId".into(),
                Value::String(session.session_id.clone()),
            )])),
            sink,
            disposed: AtomicBool::new(false),
        }
    }

    /// Original: `SessionLogService.close()`.
    pub fn close(&self) -> BoxFuture<'_, std::io::Result<()>> {
        self.sink.close()
    }
}

impl Logger for SessionLogService {
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

impl LogService for SessionLogService {
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

impl Disposable for SessionLogService {
    /// Original: `SessionLogService.dispose()`. A session scope may be dropped
    /// outside Tokio, so file content is synchronously flushed before the
    /// asynchronous close is scheduled.
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

pub fn register_session_log_service() {
    register_scoped_service(
        LifecycleScope::Session,
        LOG_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let options = accessor.get(LOG_OPTIONS_ID)?;
            let session = accessor.get(SESSION_CONTEXT_ID)?;
            let service: Arc<dyn LogService> = Arc::new(SessionLogService::new(&options, &session));
            Ok(LogServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "log",
    );
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use tokio::fs;

    use crate::{
        _base::log::{LogPayload, resolve_logging_config},
        session::session_context::{SessionContextInput, make_session_context},
    };

    use super::*;

    fn session(temp: &std::path::Path) -> SessionContext {
        make_session_context(SessionContextInput {
            session_id: "s1".into(),
            workspace_id: "workspace".into(),
            session_dir: temp.join("session").to_string_lossy().into_owned(),
            session_scope: "sessions/workspace/s1".into(),
            cwd: temp.to_string_lossy().into_owned(),
            meta_scope: None,
        })
    }

    #[tokio::test]
    async fn writes_session_lines_omits_bound_session_id_and_closes() {
        let temp = std::env::temp_dir().join(format!("session-log-{}", uuid::Uuid::new_v4()));
        let config = resolve_logging_config(
            &temp,
            &HashMap::from([("KIMI_LOG_LEVEL".into(), "debug".into())]),
        );
        let context = session(&temp);
        let service = SessionLogService::new(&config, &context);
        service.info(
            "session event",
            Some(LogPayload::Context(Map::from_iter([(
                "requestId".into(),
                Value::String("r1".into()),
            )]))),
        );
        service.flush().await.unwrap();
        let path = resolve_session_log_path(&context.session_dir);
        let text = fs::read_to_string(path).await.unwrap();
        assert!(text.contains("session event"));
        assert!(text.contains("requestId=r1"));
        assert!(!text.contains("sessionId"));

        service.close().await.unwrap();
        service.info("after-close", None);
        assert!(
            !fs::read_to_string(resolve_session_log_path(&context.session_dir))
                .await
                .unwrap()
                .contains("after-close")
        );
        let _ = fs::remove_dir_all(temp).await;
    }

    #[test]
    fn child_loggers_share_level_and_add_context() {
        let temp = std::env::temp_dir().join(format!("session-log-{}", uuid::Uuid::new_v4()));
        let service = SessionLogService::new(
            &resolve_logging_config(&temp, &HashMap::new()),
            &session(&temp),
        );
        assert_eq!(service.level(), LogLevel::Info);
        service.set_level(LogLevel::Debug);
        assert_eq!(service.level(), LogLevel::Debug);
        let child = service.child(Map::from_iter([(
            "agentId".into(),
            Value::String("main".into()),
        )]));
        child.debug("child", None);
        service.dispose().unwrap();
    }
}
