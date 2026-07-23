//! Record-level configuration section diffing.
//!
//! Original: `packages/agent-core-v2/src/app/config/sectionDiff.ts`.

use serde_json::{Map, Value};

use super::pure::deep_equal;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecordDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

// Original: diffRecords(). `serde_json::Map` retains insertion order because
// this crate enables `preserve_order`, matching JavaScript Object.keys order.
pub fn diff_records(
    previous: Option<&Map<String, Value>>,
    current: Option<&Map<String, Value>>,
) -> RecordDiff {
    let empty = Map::new();
    let previous = previous.unwrap_or(&empty);
    let current = current.unwrap_or(&empty);
    let mut diff = RecordDiff::default();

    for (key, current_value) in current {
        match previous.get(key) {
            None => diff.added.push(key.clone()),
            Some(previous_value) if !deep_equal(previous_value, current_value) => {
                diff.changed.push(key.clone());
            }
            Some(_) => {}
        }
    }
    for key in previous.keys() {
        if !current.contains_key(key) {
            diff.removed.push(key.clone());
        }
    }
    diff
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reports_added_removed_and_deeply_changed_keys_in_source_order() {
        let previous = json!({
            "same": {"nested": [1, true]},
            "changed": {"value": 1},
            "removed": false
        });
        let current = json!({
            "added": true,
            "changed": {"value": 2},
            "same": {"nested": [1.0, true]}
        });
        assert_eq!(
            diff_records(previous.as_object(), current.as_object()),
            RecordDiff {
                added: vec!["added".into()],
                removed: vec!["removed".into()],
                changed: vec!["changed".into()],
            }
        );
    }

    #[test]
    fn missing_snapshots_are_empty_records() {
        let current = json!({"a": 1, "b": 2});
        assert_eq!(
            diff_records(None, current.as_object()),
            RecordDiff {
                added: vec!["a".into(), "b".into()],
                ..RecordDiff::default()
            }
        );
        assert_eq!(diff_records(None, None), RecordDiff::default());
    }
}
