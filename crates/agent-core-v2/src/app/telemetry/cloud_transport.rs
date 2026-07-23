//! Cloud telemetry wire values and pure payload transforms.
//!
//! Original: pure helpers and wire contracts from
//! `packages/agent-core-v2/src/app/telemetry/cloudTransport.ts`.

use indexmap::IndexMap;
use serde_json::{Map, Number, Value};

pub type CloudPrimitive = Option<Value>;
pub type CloudProperties = IndexMap<String, CloudPrimitive>;
pub type CloudContext = CloudProperties;

#[derive(Clone, Debug, PartialEq)]
pub struct CloudEvent {
    pub event_id: String,
    pub device_id: Option<String>,
    pub session_id: Option<String>,
    pub event: String,
    pub timestamp: f64,
    pub properties: CloudProperties,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnrichedCloudEvent {
    pub event: CloudEvent,
    pub context: CloudContext,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CloudPayload {
    pub user_id: String,
    pub events: Vec<CloudProperties>,
}

impl CloudPayload {
    pub fn to_json_value(&self) -> Value {
        Value::Object(Map::from_iter([
            ("user_id".into(), Value::String(self.user_id.clone())),
            (
                "events".into(),
                Value::Array(
                    self.events
                        .iter()
                        .map(cloud_properties_to_json_value)
                        .collect(),
                ),
            ),
        ]))
    }
}

pub const TELEMETRY_ENDPOINT: &str = "https://telemetry-logs.kimi.com/v1/event";
pub const SERVER_EVENT_PREFIX: &str = "kfc_";
pub const USER_ID_PREFIX: &str = "kfc_device_id_";
pub const DISK_EVENT_MAX_AGE_MS: f64 = 604_800_000.0;
pub const RETRY_BACKOFFS_MS: [u64; 3] = [1_000, 4_000, 16_000];

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct CloudPayloadError {
    message: String,
}

impl CloudPayloadError {
    fn non_primitive(key: &str) -> Self {
        Self {
            message: format!("telemetry {key} must be primitive"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct TransientCloudError(pub String);

// Original: buildUserId().
pub fn build_user_id(device_id: &str) -> String {
    format!("{USER_ID_PREFIX}{device_id}")
}

// Original: buildPayload().
pub fn build_payload(
    events: &[EnrichedCloudEvent],
    device_id: &str,
) -> Result<CloudPayload, CloudPayloadError> {
    Ok(CloudPayload {
        user_id: build_user_id(device_id),
        events: events
            .iter()
            .map(|event| flatten_event(&apply_server_prefix(event)))
            .collect::<Result<_, _>>()?,
    })
}

// Original: applyServerPrefix().
pub fn apply_server_prefix(event: &EnrichedCloudEvent) -> EnrichedCloudEvent {
    if event.event.event.is_empty() || event.event.event.starts_with(SERVER_EVENT_PREFIX) {
        return event.clone();
    }
    let mut prefixed = event.clone();
    prefixed.event.event = format!("{SERVER_EVENT_PREFIX}{}", event.event.event);
    prefixed
}

// Original: flattenEvent().
pub fn flatten_event(event: &EnrichedCloudEvent) -> Result<CloudProperties, CloudPayloadError> {
    let mut output = CloudProperties::new();
    insert_primitive(
        &mut output,
        "event_id",
        Some(Value::String(event.event.event_id.clone())),
    )?;
    insert_primitive(
        &mut output,
        "device_id",
        Some(
            event
                .event
                .device_id
                .clone()
                .map_or(Value::Null, Value::String),
        ),
    )?;
    insert_primitive(
        &mut output,
        "session_id",
        Some(
            event
                .event
                .session_id
                .clone()
                .map_or(Value::Null, Value::String),
        ),
    )?;
    insert_primitive(
        &mut output,
        "event",
        Some(Value::String(event.event.event.clone())),
    )?;
    let timestamp = Number::from_f64(event.event.timestamp)
        .map(Value::Number)
        .ok_or_else(|| CloudPayloadError::non_primitive("timestamp"))?;
    insert_primitive(&mut output, "timestamp", Some(timestamp))?;
    flatten_nested(&mut output, "property", &event.event.properties)?;
    flatten_nested(&mut output, "context", &event.context)?;
    Ok(output)
}

// Original: isCloudPrimitive(). `None` is JavaScript `undefined`.
pub fn is_cloud_primitive(value: &CloudPrimitive) -> bool {
    match value {
        None | Some(Value::Null | Value::Bool(_) | Value::String(_)) => true,
        Some(Value::Number(number)) => number
            .as_f64()
            .is_some_and(|number| number.is_finite() && number.abs() <= MAX_SAFE_INTEGER),
        Some(Value::Array(_) | Value::Object(_)) => false,
    }
}

fn flatten_nested(
    target: &mut CloudProperties,
    prefix: &str,
    values: &CloudProperties,
) -> Result<(), CloudPayloadError> {
    for (key, value) in values {
        if !is_cloud_primitive(value) {
            return Err(CloudPayloadError::non_primitive(&format!("{prefix}.{key}")));
        }
        target.insert(format!("{prefix}_{key}"), value.clone());
    }
    Ok(())
}

fn insert_primitive(
    target: &mut CloudProperties,
    key: &str,
    value: CloudPrimitive,
) -> Result<(), CloudPayloadError> {
    if !is_cloud_primitive(&value) {
        return Err(CloudPayloadError::non_primitive(key));
    }
    target.insert(key.into(), value);
    Ok(())
}

pub(crate) fn cloud_properties_to_json_value(properties: &CloudProperties) -> Value {
    Value::Object(Map::from_iter(properties.iter().filter_map(
        |(key, value)| value.clone().map(|value| (key.clone(), value)),
    )))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn event(name: &str) -> EnrichedCloudEvent {
        EnrichedCloudEvent {
            event: CloudEvent {
                event_id: "event-1".into(),
                device_id: Some("dev".into()),
                session_id: None,
                event: name.into(),
                timestamp: 123.5,
                properties: CloudProperties::from([
                    ("name".into(), Some(Value::from("bash"))),
                    ("missing".into(), None),
                ]),
            },
            context: CloudContext::from([("app_name".into(), Some(Value::from("test")))]),
        }
    }

    #[test]
    fn builds_prefixed_flattened_payload_and_omits_undefined_from_json() {
        let payload = build_payload(&[event("tool.call")], "dev").unwrap();
        assert_eq!(payload.user_id, "kfc_device_id_dev");
        assert_eq!(
            payload.events[0]["event"],
            Some(Value::from("kfc_tool.call"))
        );
        assert_eq!(
            payload.events[0]["property_name"],
            Some(Value::from("bash"))
        );
        assert_eq!(payload.events[0]["property_missing"], None);
        assert_eq!(
            payload.events[0]["context_app_name"],
            Some(Value::from("test"))
        );
        assert_eq!(payload.events[0]["session_id"], Some(Value::Null));
        assert_eq!(
            payload.to_json_value(),
            json!({
                "user_id": "kfc_device_id_dev",
                "events": [{
                    "event_id": "event-1",
                    "device_id": "dev",
                    "session_id": null,
                    "event": "kfc_tool.call",
                    "timestamp": 123.5,
                    "property_name": "bash",
                    "context_app_name": "test"
                }]
            })
        );
    }

    #[test]
    fn preserves_existing_prefix_and_rejects_non_primitives_or_unsafe_numbers() {
        let prefixed = event("kfc_exit");
        assert_eq!(apply_server_prefix(&prefixed), prefixed);

        let mut invalid = event("evt");
        invalid
            .event
            .properties
            .insert("nested".into(), Some(json!({ "bad": true })));
        assert_eq!(
            build_payload(&[invalid], "dev").unwrap_err().to_string(),
            "telemetry property.nested must be primitive"
        );
        assert!(!is_cloud_primitive(&Some(Value::from(
            9_007_199_254_740_992_u64
        ))));
    }
}
