use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
};

use serde_json::Value;
use thiserror::Error;

use crate::{
    query::get_path,
    skiplist::{RangeOptions, SkipList, compare_string},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderType {
    #[default]
    Number,
    String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompoundIndexDef {
    pub group_by: String,
    pub order_by: String,
    #[serde(default)]
    pub order_type: OrderType,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompoundIndexInfo {
    pub name: String,
    pub group_by: String,
    pub order_by: String,
    pub order_type: OrderType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OrderValue {
    Number(f64),
    String(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompoundRangeEntry {
    pub key: String,
    pub order_value: OrderValue,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CompoundIndexError {
    #[error("compound index \"{0}\" already exists")]
    AlreadyExists(String),
    #[error("no such compound index: {0}")]
    NotFound(String),
}

struct CompoundEntry {
    info: CompoundIndexInfo,
    groups: HashMap<String, SkipList<OrderValue, String>>,
    by_pk: HashMap<String, (String, OrderValue)>,
}

#[derive(Default)]
pub struct CompoundIndexManager {
    indexes: HashMap<String, CompoundEntry>,
}

impl CompoundIndexManager {
    // Original: packages/minidb/src/compound-index.ts, CompoundIndexManager.create().
    pub fn create(
        &mut self,
        name: impl Into<String>,
        def: CompoundIndexDef,
    ) -> Result<(), CompoundIndexError> {
        let name = name.into();
        if self.indexes.contains_key(&name) {
            return Err(CompoundIndexError::AlreadyExists(name));
        }
        let info = CompoundIndexInfo {
            name: name.clone(),
            group_by: def.group_by,
            order_by: def.order_by,
            order_type: def.order_type,
        };
        self.indexes.insert(
            name,
            CompoundEntry {
                info,
                groups: HashMap::new(),
                by_pk: HashMap::new(),
            },
        );
        Ok(())
    }

    pub fn drop(&mut self, name: &str) -> bool {
        self.indexes.remove(name).is_some()
    }

    pub fn list(&self) -> Vec<CompoundIndexInfo> {
        self.indexes
            .values()
            .map(|entry| entry.info.clone())
            .collect()
    }

    // Original: CompoundIndexManager.add(). Datetime metadata takes precedence over document fields.
    pub fn add(&mut self, pk: &str, doc: &Value, datetimes: Option<&BTreeMap<String, i64>>) {
        for entry in self.indexes.values_mut() {
            let group = get_path(doc, &entry.info.group_by)
                .filter(|value| !value.is_null())
                .map(canonical_value);
            let order = datetimes
                .and_then(|values| values.get(&entry.info.order_by))
                .map(|value| OrderValue::Number(*value as f64))
                .or_else(|| {
                    get_path(doc, &entry.info.order_by)
                        .and_then(|value| order_value(value, entry.info.order_type))
                });
            let placement = group
                .zip(order)
                .filter(|(_, order)| valid_order(order, entry.info.order_type));
            if let (Some(previous), Some(next)) = (entry.by_pk.get(pk), placement.as_ref())
                && previous == next
            {
                continue;
            }
            if let Some((group, order)) = entry.by_pk.remove(pk)
                && let Some(list) = entry.groups.get_mut(&group)
            {
                list.delete(&order, &pk.to_owned());
                if list.is_empty() {
                    entry.groups.remove(&group);
                }
            }
            if let Some((group, order)) = placement {
                entry
                    .groups
                    .entry(group.clone())
                    .or_insert_with(|| SkipList::with_comparators(compare_order, compare_string))
                    .insert(order.clone(), pk.to_owned());
                entry.by_pk.insert(pk.to_owned(), (group, order));
            }
        }
    }

    pub fn remove(&mut self, pk: &str) {
        for entry in self.indexes.values_mut() {
            if let Some((group, order)) = entry.by_pk.remove(pk)
                && let Some(list) = entry.groups.get_mut(&group)
            {
                list.delete(&order, &pk.to_owned());
            }
        }
    }

    pub fn range(
        &self,
        name: &str,
        group: &Value,
        options: &RangeOptions<OrderValue>,
    ) -> Result<Vec<CompoundRangeEntry>, CompoundIndexError> {
        let entry = self
            .indexes
            .get(name)
            .ok_or_else(|| CompoundIndexError::NotFound(name.into()))?;
        let Some(list) = entry.groups.get(&canonical_value(group)) else {
            return Ok(Vec::new());
        };
        Ok(list
            .range(options)
            .into_iter()
            .map(|item| CompoundRangeEntry {
                key: item.value,
                order_value: item.key,
            })
            .collect())
    }

    pub fn rebuild<'a>(
        &mut self,
        entries: impl IntoIterator<Item = (&'a str, &'a Value, Option<&'a BTreeMap<String, i64>>)>,
    ) {
        for entry in self.indexes.values_mut() {
            entry.groups.clear();
            entry.by_pk.clear();
        }
        for (key, value, datetimes) in entries {
            self.add(key, value, datetimes);
        }
    }
}

fn compare_order(left: &OrderValue, right: &OrderValue) -> Ordering {
    match (left, right) {
        (OrderValue::Number(left), OrderValue::Number(right)) => {
            left.partial_cmp(right).unwrap_or(Ordering::Equal)
        }
        (OrderValue::String(left), OrderValue::String(right)) => left.cmp(right),
        (OrderValue::Number(_), OrderValue::String(_)) => Ordering::Less,
        (OrderValue::String(_), OrderValue::Number(_)) => Ordering::Greater,
    }
}

fn order_value(value: &Value, order_type: OrderType) -> Option<OrderValue> {
    match order_type {
        OrderType::Number => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(OrderValue::Number),
        OrderType::String => value.as_str().map(|value| OrderValue::String(value.into())),
    }
}

fn valid_order(value: &OrderValue, order_type: OrderType) -> bool {
    matches!((value, order_type), (OrderValue::Number(value), OrderType::Number) if value.is_finite())
        || matches!(
            (value, order_type),
            (OrderValue::String(_), OrderType::String)
        )
}

fn canonical_value(value: &Value) -> String {
    match value {
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_value)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            format!(
                "{{{}}}",
                keys.into_iter()
                    .map(|key| format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap(),
                        canonical_value(&map[key])
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        _ => serde_json::to_string(value).unwrap(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_orders_updates_and_removes_documents() {
        let mut indexes = CompoundIndexManager::default();
        indexes
            .create(
                "workspace_updated",
                CompoundIndexDef {
                    group_by: "workspaceId".into(),
                    order_by: "updatedAt".into(),
                    order_type: OrderType::Number,
                },
            )
            .unwrap();
        indexes.add(
            "b",
            &serde_json::json!({"workspaceId":"w", "updatedAt":2}),
            None,
        );
        indexes.add(
            "a",
            &serde_json::json!({"workspaceId":"w", "updatedAt":1}),
            None,
        );
        assert_eq!(
            indexes
                .range(
                    "workspace_updated",
                    &Value::from("w"),
                    &RangeOptions::default()
                )
                .unwrap()
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        indexes.add(
            "a",
            &serde_json::json!({"workspaceId":"w", "updatedAt":3}),
            None,
        );
        indexes.remove("b");
        assert_eq!(
            indexes
                .range(
                    "workspace_updated",
                    &Value::from("w"),
                    &RangeOptions::default()
                )
                .unwrap()[0]
                .order_value,
            OrderValue::Number(3.0)
        );
    }
}
