//! Wire journal protocol migrations.
//!
//! Original: `packages/agent-core-v2/src/wire/migration/migration.ts`.

use std::cmp::Ordering;

use serde_json::{Map, Value};

use super::record::WireRecord;

pub const WIRE_PROTOCOL_VERSION: &str = "1.5";

#[derive(Clone, Copy, Debug)]
pub struct WireMigration {
    pub source_version: &'static str,
    pub target_version: &'static str,
    pub migrate_record: fn(&WireRecord) -> WireRecord,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Missing wire migration for version {version}")]
pub struct MissingWireMigrationError {
    pub version: String,
}

pub const MIGRATE_V1_0_TO_V1_1: WireMigration = WireMigration {
    source_version: "1.0",
    target_version: "1.1",
    migrate_record: migrate_v1_0_record,
};
pub const MIGRATE_V1_1_TO_V1_2: WireMigration = WireMigration {
    source_version: "1.1",
    target_version: "1.2",
    migrate_record: migrate_v1_1_record,
};
pub const MIGRATE_V1_2_TO_V1_3: WireMigration = WireMigration {
    source_version: "1.2",
    target_version: "1.3",
    migrate_record: clone_record,
};
pub const MIGRATE_V1_3_TO_V1_4: WireMigration = WireMigration {
    source_version: "1.3",
    target_version: "1.4",
    migrate_record: migrate_v1_3_record,
};
pub const MIGRATE_V1_4_TO_V1_5: WireMigration = WireMigration {
    source_version: "1.4",
    target_version: "1.5",
    migrate_record: migrate_v1_4_record,
};

const MIGRATIONS: &[WireMigration] = &[
    MIGRATE_V1_0_TO_V1_1,
    MIGRATE_V1_1_TO_V1_2,
    MIGRATE_V1_2_TO_V1_3,
    MIGRATE_V1_3_TO_V1_4,
    MIGRATE_V1_4_TO_V1_5,
];

pub fn is_newer_wire_version(read_version: &str) -> bool {
    compare_wire_versions(read_version, WIRE_PROTOCOL_VERSION) == Some(Ordering::Greater)
}

pub fn resolve_wire_migrations(
    read_version: &str,
) -> Result<Vec<WireMigration>, MissingWireMigrationError> {
    if compare_wire_versions(read_version, WIRE_PROTOCOL_VERSION) != Some(Ordering::Less) {
        return Ok(Vec::new());
    }
    let mut migrations = Vec::new();
    let mut version = read_version;
    while compare_wire_versions(version, WIRE_PROTOCOL_VERSION) == Some(Ordering::Less) {
        let migration = MIGRATIONS
            .iter()
            .find(|migration| migration.source_version == version)
            .copied()
            .ok_or_else(|| MissingWireMigrationError {
                version: version.into(),
            })?;
        migrations.push(migration);
        version = migration.target_version;
    }
    Ok(migrations)
}

pub fn migrate_wire_record(record: &WireRecord, migrations: &[WireMigration]) -> WireRecord {
    migrations
        .iter()
        .fold(record.clone(), |current, migration| {
            (migration.migrate_record)(&current)
        })
}

pub fn migrate_wire_records(
    records: &[WireRecord],
    read_version: Option<&str>,
) -> Result<Vec<WireRecord>, MissingWireMigrationError> {
    let migrations = match read_version {
        Some(version) => resolve_wire_migrations(version)?,
        None => MIGRATIONS.to_vec(),
    };
    Ok(apply_wire_migrations(records, &migrations))
}

pub fn apply_wire_migrations(
    records: &[WireRecord],
    migrations: &[WireMigration],
) -> Vec<WireRecord> {
    records
        .iter()
        .map(|record| migrate_wire_record(record, migrations))
        .collect()
}

fn compare_wire_versions(left: &str, right: &str) -> Option<Ordering> {
    let left = parse_version(left)?;
    let right = parse_version(right)?;
    let length = left.len().max(right.len());
    for index in 0..length {
        let ordering = left
            .get(index)
            .copied()
            .unwrap_or_default()
            .cmp(&right.get(index).copied().unwrap_or_default());
        if ordering != Ordering::Equal {
            return Some(ordering);
        }
    }
    Some(Ordering::Equal)
}

fn parse_version(version: &str) -> Option<Vec<i64>> {
    version
        .split('.')
        .map(|part| part.parse::<i64>().ok())
        .collect()
}

fn clone_record(record: &WireRecord) -> WireRecord {
    record.clone()
}

// Original: migrateV1_0ToV1_1.migrateRecord().
fn migrate_v1_0_record(record: &WireRecord) -> WireRecord {
    if record_type(record) != Some("context.append_message") {
        return record.clone();
    }
    let mut migrated = record.clone();
    let Some(message) = migrated.get_mut("message").and_then(Value::as_object_mut) else {
        return record.clone();
    };
    let Some(tool_calls) = message.get_mut("toolCalls").and_then(Value::as_array_mut) else {
        return record.clone();
    };
    for tool_call in tool_calls {
        let Some(tool_call) = tool_call.as_object_mut() else {
            continue;
        };
        let Some(Value::Object(function)) = tool_call.remove("function") else {
            continue;
        };
        if let Some(name) = function.get("name") {
            tool_call.insert("name".into(), name.clone());
        }
        if let Some(arguments) = function.get("arguments") {
            tool_call.insert("arguments".into(), arguments.clone());
        }
    }
    migrated
}

// Original: migrateV1_1ToV1_2.migrateRecord().
fn migrate_v1_1_record(record: &WireRecord) -> WireRecord {
    if record_type(record) != Some("permission.record_approval_result")
        || record.contains_key("sessionApprovalRule")
    {
        return record.clone();
    }
    let Some(result) = record.get("result").and_then(Value::as_object) else {
        return record.clone();
    };
    if result.get("decision").and_then(Value::as_str) != Some("approved")
        || result.get("scope").and_then(Value::as_str) != Some("session")
    {
        return record.clone();
    }
    let action = record
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if matches!(
        action,
        "run command in plan mode" | "run background command"
    ) {
        return record.clone();
    }
    let pattern = match action {
        "run command" => Some("Bash"),
        "stop background task" => Some("TaskStop"),
        "edit file" | "edit file outside of working directory" | "write file" => Some("Write"),
        _ => record.get("toolName").and_then(Value::as_str),
    };
    let Some(pattern) = pattern else {
        return record.clone();
    };
    let mut migrated = record.clone();
    migrated.insert("sessionApprovalRule".into(), Value::String(pattern.into()));
    migrated
}

// Original: migrateV1_3ToV1_4.migrateRecord(). These migrations intentionally
// discard stale goalId and unknown fields from the listed historical records.
fn migrate_v1_3_record(record: &WireRecord) -> WireRecord {
    match record_type(record) {
        Some("goal.create") => select_fields(
            record,
            "goal.create",
            &["goalId", "objective", "completionCriterion", "time"],
        ),
        Some("goal.update") => select_fields(
            record,
            "goal.update",
            &[
                "status",
                "reason",
                "turnsUsed",
                "tokensUsed",
                "wallClockMs",
                "actor",
                "time",
            ],
        ),
        Some("goal.account_usage") => select_fields(
            record,
            "goal.update",
            &["tokensUsed", "wallClockMs", "time"],
        ),
        Some("goal.continuation") => select_fields(record, "goal.update", &["turnsUsed", "time"]),
        Some("goal.clear") => select_fields(record, "goal.clear", &["time"]),
        _ => record.clone(),
    }
}

fn select_fields(record: &WireRecord, record_type: &str, fields: &[&str]) -> WireRecord {
    let mut selected = Map::new();
    selected.insert("type".into(), Value::String(record_type.into()));
    for field in fields {
        if let Some(value) = record.get(*field) {
            selected.insert((*field).into(), value.clone());
        }
    }
    selected
}

// Original: migrateV1_4ToV1_5.migrateRecord().
fn migrate_v1_4_record(record: &WireRecord) -> WireRecord {
    if !advances_active_interval(record)
        || record.contains_key("wallClockResumedAt")
        || !record.get("time").is_some_and(Value::is_number)
    {
        return record.clone();
    }
    let mut migrated = record.clone();
    migrated.insert(
        "wallClockResumedAt".into(),
        record.get("time").cloned().unwrap_or(Value::Null),
    );
    migrated
}

fn advances_active_interval(record: &WireRecord) -> bool {
    record_type(record) == Some("goal.create")
        || (record_type(record) == Some("goal.update")
            && (record.get("status").and_then(Value::as_str) == Some("active")
                || (!record.contains_key("status")
                    && record.get("wallClockMs").is_some_and(Value::is_number))))
}

fn record_type(record: &WireRecord) -> Option<&str> {
    record.get("type").and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(value: Value) -> WireRecord {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn resolves_versions_in_order_and_rejects_missing_paths() {
        let migrations = resolve_wire_migrations("1.0").unwrap();
        assert_eq!(migrations.len(), 5);
        assert_eq!(migrations[0].target_version, "1.1");
        assert_eq!(migrations[4].target_version, "1.5");
        assert!(resolve_wire_migrations("1.5").unwrap().is_empty());
        assert!(is_newer_wire_version("2.0"));
        assert!(!is_newer_wire_version("invalid"));
        assert_eq!(resolve_wire_migrations("0.9").unwrap_err().version, "0.9");
    }

    #[test]
    fn v1_1_flattens_only_context_message_tool_calls() {
        let input = record(serde_json::json!({
            "type": "context.append_message",
            "message": {"toolCalls": [{
                "type": "function",
                "id": "call-1",
                "function": {"name": "Bash", "arguments": "{}"}
            }]}
        }));
        let migrated = (MIGRATE_V1_0_TO_V1_1.migrate_record)(&input);
        assert_eq!(
            migrated["message"]["toolCalls"][0],
            serde_json::json!({
                "type": "function", "id": "call-1", "name": "Bash", "arguments": "{}"
            })
        );
    }

    #[test]
    fn v1_2_recovers_only_restorable_session_approval_patterns() {
        let approved = record(serde_json::json!({
            "type": "permission.record_approval_result",
            "toolName": "Shell", "action": "run command",
            "result": {"decision": "approved", "scope": "session"}
        }));
        let migrated = (MIGRATE_V1_1_TO_V1_2.migrate_record)(&approved);
        assert_eq!(migrated["sessionApprovalRule"], "Bash");
        let mut unrestorable = approved;
        unrestorable.insert(
            "action".into(),
            Value::String("run background command".into()),
        );
        assert!(
            !(MIGRATE_V1_1_TO_V1_2.migrate_record)(&unrestorable)
                .contains_key("sessionApprovalRule")
        );
    }

    #[test]
    fn v1_4_rewrites_goal_records_and_drops_retired_identity() {
        let update = record(serde_json::json!({
            "type": "goal.account_usage", "goalId": "old", "tokensUsed": 4,
            "wallClockMs": 10, "time": 20, "unknown": true
        }));
        assert_eq!(
            Value::Object((MIGRATE_V1_3_TO_V1_4.migrate_record)(&update)),
            serde_json::json!({
                "type": "goal.update", "tokensUsed": 4, "wallClockMs": 10, "time": 20
            })
        );
    }

    #[test]
    fn v1_5_adds_missing_active_interval_anchors_only() {
        let create = record(serde_json::json!({"type": "goal.create", "time": 42}));
        assert_eq!(
            (MIGRATE_V1_4_TO_V1_5.migrate_record)(&create)["wallClockResumedAt"],
            42
        );
        let paused = record(serde_json::json!({
            "type": "goal.update", "status": "paused", "time": 42
        }));
        assert_eq!((MIGRATE_V1_4_TO_V1_5.migrate_record)(&paused), paused);
        let checkpoint = record(serde_json::json!({
            "type": "goal.update", "wallClockMs": 9, "time": 43
        }));
        assert_eq!(
            (MIGRATE_V1_4_TO_V1_5.migrate_record)(&checkpoint)["wallClockResumedAt"],
            43
        );
    }

    #[test]
    fn migrate_wire_record_applies_custom_steps_in_order() {
        fn first(record: &WireRecord) -> WireRecord {
            let mut record = record.clone();
            record.insert("first".into(), Value::Bool(true));
            record
        }
        fn second(record: &WireRecord) -> WireRecord {
            let mut record = record.clone();
            record.insert(
                "second".into(),
                Value::Bool(record.get("first") == Some(&Value::Bool(true))),
            );
            record
        }
        let migrations = [
            WireMigration {
                source_version: "0.8",
                target_version: "0.9",
                migrate_record: first,
            },
            WireMigration {
                source_version: "0.9",
                target_version: "1.0",
                migrate_record: second,
            },
        ];
        let migrated = migrate_wire_record(
            &record(serde_json::json!({"type": "metadata"})),
            &migrations,
        );
        assert_eq!(migrated.get("second"), Some(&Value::Bool(true)));
    }
}
