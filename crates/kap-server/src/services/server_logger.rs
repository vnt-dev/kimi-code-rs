use std::io::Write;
use std::sync::{Arc, Mutex};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ServerLogLevel {
    Fatal,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
    Silent,
}

impl ServerLogLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fatal => "fatal",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
            Self::Silent => "silent",
        }
    }
}

pub trait ServerLogger: Send + Sync {
    fn log(&self, level: ServerLogLevel, fields: serde_json::Value, message: &str);
}

pub struct JsonServerLogger {
    level: ServerLogLevel,
    writer: Mutex<Box<dyn Write + Send>>,
}

impl std::fmt::Debug for JsonServerLogger {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JsonServerLogger")
            .field("level", &self.level)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
struct LogLine<'a> {
    level: &'static str,
    time: String,
    name: &'static str,
    msg: &'a str,
    #[serde(flatten)]
    fields: serde_json::Value,
}

// Original: pinoLoggerService.ts, createServerLogger().
pub fn create_server_logger(level: ServerLogLevel) -> Arc<dyn ServerLogger> {
    Arc::new(JsonServerLogger {
        level,
        writer: Mutex::new(Box::new(std::io::stderr())),
    })
}

impl JsonServerLogger {
    #[cfg(test)]
    fn with_writer(level: ServerLogLevel, writer: Box<dyn Write + Send>) -> Self {
        Self {
            level,
            writer: Mutex::new(writer),
        }
    }

    fn enabled(&self, level: ServerLogLevel) -> bool {
        self.level != ServerLogLevel::Silent && level <= self.level
    }
}

impl ServerLogger for JsonServerLogger {
    fn log(&self, level: ServerLogLevel, fields: serde_json::Value, message: &str) {
        if !self.enabled(level) {
            return;
        }
        let line = LogLine {
            level: level.as_str(),
            time: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            name: "kimi-server-v2",
            msg: message,
            fields,
        };
        if let Ok(serialized) = serde_json::to_vec(&line) {
            let mut writer = self
                .writer
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let _ = writer.write_all(&serialized);
            let _ = writer.write_all(b"\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writes_newline_delimited_json_at_configured_level() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let logger = JsonServerLogger::with_writer(
            ServerLogLevel::Info,
            Box::new(SharedWriter(Arc::clone(&output))),
        );
        logger.log(
            ServerLogLevel::Debug,
            serde_json::json!({"hidden": true}),
            "hidden",
        );
        logger.log(
            ServerLogLevel::Info,
            serde_json::json!({"request_id": "r"}),
            "request completed",
        );
        let bytes = output.lock().unwrap().clone();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.ends_with('\n'));
        let line: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(line["level"], "info");
        assert_eq!(line["name"], "kimi-server-v2");
        assert_eq!(line["request_id"], "r");
    }
}
