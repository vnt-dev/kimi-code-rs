//! Development console telemetry appender.
//!
//! Original: `packages/agent-core-v2/src/app/telemetry/consoleAppender.ts`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value};

use super::contract::{TelemetryAppender, TelemetryProperties};

const DEFAULT_PREFIX: &str = "[telemetry]";

#[derive(Clone)]
pub struct ConsoleAppenderOptions {
    pub prefix: String,
    pub pretty: bool,
    pub log: Arc<dyn Fn(&str) + Send + Sync>,
}

impl Default for ConsoleAppenderOptions {
    fn default() -> Self {
        Self {
            prefix: DEFAULT_PREFIX.into(),
            pretty: false,
            log: Arc::new(|message| println!("{message}")),
        }
    }
}

pub struct ConsoleAppender {
    options: ConsoleAppenderOptions,
}

impl ConsoleAppender {
    // Original: ConsoleAppender.constructor().
    pub fn new(options: ConsoleAppenderOptions) -> Self {
        Self { options }
    }
}

impl Default for ConsoleAppender {
    fn default() -> Self {
        Self::new(ConsoleAppenderOptions::default())
    }
}

#[async_trait]
impl TelemetryAppender for ConsoleAppender {
    // Original: ConsoleAppender.track().
    fn track(&self, event: &str, properties: Option<&TelemetryProperties>) {
        let payload = properties.map_or_else(String::new, |properties| {
            format!(" {}", stringify_properties(properties, self.options.pretty))
        });
        (self.options.log)(&format!("{} {event}{payload}", self.options.prefix));
    }
}

fn stringify_properties(properties: &TelemetryProperties, pretty: bool) -> String {
    let properties =
        Value::Object(Map::from_iter(properties.iter().filter_map(
            |(key, value)| value.as_ref().map(|value| (key.clone(), value.clone())),
        )));
    if pretty {
        serde_json::to_string_pretty(&properties).expect("telemetry primitives are serializable")
    } else {
        serde_json::to_string(&properties).expect("telemetry primitives are serializable")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use indexmap::IndexMap;
    use serde_json::json;

    use super::*;

    #[test]
    fn writes_default_compact_format_and_omits_undefined_properties() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&messages);
        let appender = ConsoleAppender::new(ConsoleAppenderOptions {
            log: Arc::new(move |message| output.lock().unwrap().push(message.to_owned())),
            ..ConsoleAppenderOptions::default()
        });
        appender.track("turn_started", None);
        appender.track(
            "turn_ended",
            Some(&IndexMap::from([
                ("duration_ms".into(), Some(json!(12))),
                ("trace_id".into(), None),
                ("error".into(), Some(Value::Null)),
            ])),
        );
        assert_eq!(
            *messages.lock().unwrap(),
            [
                "[telemetry] turn_started",
                "[telemetry] turn_ended {\"duration_ms\":12,\"error\":null}",
            ]
        );
    }

    #[test]
    fn supports_custom_prefix_and_pretty_json() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&messages);
        let appender = ConsoleAppender::new(ConsoleAppenderOptions {
            prefix: "[metrics]".into(),
            pretty: true,
            log: Arc::new(move |message| output.lock().unwrap().push(message.to_owned())),
        });
        appender.track(
            "event",
            Some(&IndexMap::from([("enabled".into(), Some(json!(true)))])),
        );
        assert_eq!(
            messages.lock().unwrap()[0],
            "[metrics] event {\n  \"enabled\": true\n}"
        );
    }
}
