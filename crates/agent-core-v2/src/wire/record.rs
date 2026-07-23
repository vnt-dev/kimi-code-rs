//! Pure encoding and decoding for the persisted JSONL wire record language.
//!
//! Original: `packages/agent-core-v2/src/wire/record.ts`.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::migration::WIRE_PROTOCOL_VERSION;

pub const AGENT_WIRE_RECORD_KEY: &str = "wire.jsonl";

pub type WireRecord = Map<String, Value>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WireMetadataRecord {
    #[serde(rename = "type")]
    pub record_type: String,
    pub protocol_version: String,
    pub created_at: i64,
}

impl WireMetadataRecord {
    pub fn into_wire_record(self) -> WireRecord {
        [
            ("type".into(), Value::String(self.record_type)),
            (
                "protocol_version".into(),
                Value::String(self.protocol_version),
            ),
            ("created_at".into(), Value::from(self.created_at)),
        ]
        .into_iter()
        .collect()
    }
}

pub fn is_wire_record(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|record| record.get("type"))
        .is_some_and(Value::is_string)
}

pub fn create_wire_metadata_record() -> WireMetadataRecord {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    create_wire_metadata_record_at(i64::try_from(now).unwrap_or(i64::MAX))
}

pub fn create_wire_metadata_record_at(now_millis: i64) -> WireMetadataRecord {
    WireMetadataRecord {
        record_type: "metadata".into(),
        protocol_version: WIRE_PROTOCOL_VERSION.into(),
        created_at: now_millis,
    }
}

pub fn is_wire_metadata_record(record: &WireRecord) -> bool {
    record.get("type").and_then(Value::as_str) == Some("metadata")
        && record.get("protocol_version").is_some_and(Value::is_string)
        && record.get("created_at").is_some_and(Value::is_number)
}

pub fn op_to_wire_record(op_type: &str, payload: &Value) -> WireRecord {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    op_to_wire_record_at(op_type, payload, i64::try_from(now).unwrap_or(i64::MAX))
}

// Original: opToWireRecord(). Object payload fields are spread after `type`,
// so payload-owned `type` and `time` intentionally retain source precedence.
pub fn op_to_wire_record_at(op_type: &str, payload: &Value, now_millis: i64) -> WireRecord {
    let mut record = [("type".into(), Value::String(op_type.into()))]
        .into_iter()
        .collect::<WireRecord>();
    if let Value::Object(fields) = payload {
        record.extend(fields.clone());
    } else {
        record.insert("payload".into(), payload.clone());
    }
    record
        .entry("time")
        .or_insert_with(|| Value::from(now_millis));
    record
}

// Original: wireRecordToPayload().
pub fn wire_record_to_payload(record: &WireRecord) -> Value {
    let mut payload = record.clone();
    payload.remove("type");
    payload.remove("time");
    if payload.len() == 1 && payload.contains_key("payload") {
        payload.remove("payload").unwrap_or(Value::Null)
    } else {
        Value::Object(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_records_and_metadata_envelopes() {
        assert!(is_wire_record(&serde_json::json!({"type": "goal.create"})));
        assert!(!is_wire_record(&serde_json::json!([])));
        assert!(!is_wire_record(&serde_json::json!({"type": 1})));

        let metadata = create_wire_metadata_record_at(123);
        assert_eq!(metadata.protocol_version, "1.5");
        let record = metadata.into_wire_record();
        assert!(is_wire_metadata_record(&record));
        assert_eq!(record.get("created_at"), Some(&Value::from(123)));
    }

    #[test]
    fn object_payloads_flatten_and_preserve_payload_field_precedence() {
        let record = op_to_wire_record_at(
            "goal.create",
            &serde_json::json!({"goal_id": "g1", "type": "payload.type", "time": 7}),
            99,
        );
        assert_eq!(
            record.get("type").and_then(Value::as_str),
            Some("payload.type")
        );
        assert_eq!(record.get("time"), Some(&Value::from(7)));
        assert_eq!(
            wire_record_to_payload(&record),
            serde_json::json!({"goal_id": "g1"})
        );
    }

    #[test]
    fn scalar_array_null_and_single_payload_objects_round_trip() {
        for payload in [
            serde_json::json!("text"),
            serde_json::json!([1, 2]),
            Value::Null,
        ] {
            let record = op_to_wire_record_at("test", &payload, 1);
            assert_eq!(wire_record_to_payload(&record), payload);
        }
        let payload = serde_json::json!({"payload": 42});
        let record = op_to_wire_record_at("test", &payload, 1);
        assert_eq!(wire_record_to_payload(&record), Value::from(42));
    }
}
