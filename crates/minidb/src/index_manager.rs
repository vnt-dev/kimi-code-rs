use std::collections::{HashMap, HashSet};

use serde_json::Value;
use thiserror::Error;

use crate::{
    query::get_path,
    skiplist::{RangeOptions, SkipList, compare_number, compare_string},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexType {
    Equality,
    Range,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDef {
    pub field: String,
    pub index_type: IndexType,
    pub unique: bool,
    pub sparse: bool,
}

impl IndexDef {
    pub fn equality(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            index_type: IndexType::Equality,
            unique: false,
            sparse: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexInfo {
    pub name: String,
    pub field: String,
    pub index_type: IndexType,
    pub unique: bool,
    pub sparse: bool,
}

struct EqualityIndex {
    info: IndexInfo,
    map: HashMap<String, Vec<String>>,
    by_pk: HashMap<String, Vec<String>>,
}

struct RangeIndex {
    info: IndexInfo,
    list: SkipList<f64, String>,
    by_pk: HashMap<String, Vec<f64>>,
}

enum AnyIndex {
    Equality(EqualityIndex),
    Range(RangeIndex),
}

impl AnyIndex {
    fn info(&self) -> &IndexInfo {
        match self {
            Self::Equality(index) => &index.info,
            Self::Range(index) => &index.info,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IndexError {
    #[error("index requires a field")]
    MissingField,
    #[error("index \"{0}\" already exists")]
    AlreadyExists(String),
    #[error("no such index: {0}")]
    NotFound(String),
    #[error("index \"{0}\" is not an equality index")]
    NotEquality(String),
    #[error("index \"{0}\" is not a range index")]
    NotRange(String),
    #[error("unique index \"{index}\" violation on value {value}")]
    UniqueViolation { index: String, value: String },
}

#[derive(Debug, Clone)]
pub struct BatchIndexOp {
    pub pk: String,
    pub op: BatchIndexOpType,
    pub doc: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchIndexOpType {
    Set,
    Del,
}

#[derive(Debug, Clone, Default)]
pub struct NumericRangeOptions {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub min_exclusive: bool,
    pub max_exclusive: bool,
    pub offset: usize,
    pub count: Option<usize>,
    pub reverse: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NumericRangeEntry {
    pub pk: String,
    pub value: f64,
}

#[derive(Default)]
pub struct IndexManager {
    indexes: HashMap<String, AnyIndex>,
}

impl IndexManager {
    // Original: packages/minidb/src/index-manager.ts, IndexManager.create().
    pub fn create(&mut self, name: impl Into<String>, def: IndexDef) -> Result<(), IndexError> {
        let name = name.into();
        if def.field.is_empty() {
            return Err(IndexError::MissingField);
        }
        if self.indexes.contains_key(&name) {
            return Err(IndexError::AlreadyExists(name));
        }
        let info = IndexInfo {
            name: name.clone(),
            field: def.field,
            index_type: def.index_type,
            unique: def.unique,
            sparse: def.sparse,
        };
        let index = match info.index_type {
            IndexType::Equality => AnyIndex::Equality(EqualityIndex {
                info,
                map: HashMap::new(),
                by_pk: HashMap::new(),
            }),
            IndexType::Range => AnyIndex::Range(RangeIndex {
                info,
                list: SkipList::with_comparators(compare_number, compare_string),
                by_pk: HashMap::new(),
            }),
        };
        self.indexes.insert(name, index);
        Ok(())
    }

    pub fn drop(&mut self, name: &str) -> bool {
        self.indexes.remove(name).is_some()
    }

    pub fn list(&self) -> Vec<IndexInfo> {
        self.indexes
            .values()
            .map(|index| index.info().clone())
            .collect()
    }

    pub fn check_unique(&self, pk: &str, doc: &Value) -> Result<(), IndexError> {
        for index in self.indexes.values() {
            if !index.info().unique {
                continue;
            }
            let value = get_path(doc, &index.info().field);
            if value.is_none() && index.info().sparse {
                continue;
            }
            for item in flatten(value.unwrap_or(&Value::Null)) {
                match index {
                    AnyIndex::Range(index) => {
                        let Some(number) = item.as_f64().filter(|number| number.is_finite()) else {
                            continue;
                        };
                        if let Some(hit) = index
                            .list
                            .range(&RangeOptions {
                                gte: Some(number),
                                lte: Some(number),
                                count: Some(1),
                                ..Default::default()
                            })
                            .first()
                            && hit.value != pk
                        {
                            return Err(unique_error(&index.info.name, item));
                        }
                    }
                    AnyIndex::Equality(index) => {
                        if let Some(keys) = index.map.get(&scalar_key(item))
                            && (keys.len() > 1 || (keys.len() == 1 && keys[0] != pk))
                        {
                            return Err(unique_error(&index.info.name, item));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // Original: IndexManager.checkUniqueBatch(). Final-state validation intentionally ignores operation order.
    pub fn check_unique_batch(&self, operations: &[BatchIndexOp]) -> Result<(), IndexError> {
        let mut last = HashMap::<&str, &BatchIndexOp>::new();
        for operation in operations {
            last.insert(&operation.pk, operation);
        }
        let touched = last.keys().copied().collect::<HashSet<_>>();
        for index in self.indexes.values() {
            if !index.info().unique {
                continue;
            }
            match index {
                AnyIndex::Range(index) => {
                    let mut owners = HashMap::<u64, &str>::new();
                    for (pk, values) in &index.by_pk {
                        if touched.contains(pk.as_str()) {
                            continue;
                        }
                        for value in values {
                            owners.insert(value.to_bits(), pk);
                        }
                    }
                    for (pk, operation) in &last {
                        if operation.op == BatchIndexOpType::Del {
                            continue;
                        }
                        for value in flatten(
                            get_path(&operation.doc, &index.info.field).unwrap_or(&Value::Null),
                        ) {
                            let Some(number) = value.as_f64().filter(|number| number.is_finite())
                            else {
                                continue;
                            };
                            if let Some(previous) = owners.insert(number.to_bits(), pk)
                                && previous != *pk
                            {
                                return Err(unique_error(&index.info.name, value));
                            }
                        }
                    }
                }
                AnyIndex::Equality(index) => {
                    let mut owners = HashMap::<String, &str>::new();
                    for (value, keys) in &index.map {
                        for pk in keys {
                            if !touched.contains(pk.as_str()) {
                                owners.insert(value.clone(), pk);
                            }
                        }
                    }
                    for (pk, operation) in &last {
                        if operation.op == BatchIndexOpType::Del {
                            continue;
                        }
                        let value = get_path(&operation.doc, &index.info.field);
                        if value.is_none() && index.info.sparse {
                            continue;
                        }
                        for item in flatten(value.unwrap_or(&Value::Null)) {
                            if let Some(previous) = owners.insert(scalar_key(item), pk)
                                && previous != *pk
                            {
                                return Err(unique_error(&index.info.name, item));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn assert_unique_valid(&self, name: &str) -> Result<(), IndexError> {
        let index = self
            .indexes
            .get(name)
            .ok_or_else(|| IndexError::NotFound(name.into()))?;
        if !index.info().unique {
            return Ok(());
        }
        match index {
            AnyIndex::Range(index) => {
                let mut owners = HashMap::<u64, &str>::new();
                for (pk, values) in &index.by_pk {
                    for value in values {
                        if let Some(previous) = owners.insert(value.to_bits(), pk)
                            && previous != pk
                        {
                            return Err(unique_error(&index.info.name, &Value::from(*value)));
                        }
                    }
                }
            }
            AnyIndex::Equality(index) => {
                for keys in index.map.values() {
                    if keys.len() > 1 {
                        return Err(IndexError::UniqueViolation {
                            index: index.info.name.clone(),
                            value: serde_json::to_string(&format!(
                                "{} keys (e.g. {})",
                                keys.len(),
                                keys[0]
                            ))
                            .unwrap(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    pub fn add(&mut self, pk: &str, doc: &Value) {
        for index in self.indexes.values_mut() {
            let value = get_path(doc, &index.info().field);
            if value.is_none() && index.info().sparse {
                continue;
            }
            match index {
                AnyIndex::Range(index) => {
                    let mut seen = HashSet::new();
                    let values = flatten(value.unwrap_or(&Value::Null))
                        .into_iter()
                        .filter_map(|value| value.as_f64())
                        .filter(|value| value.is_finite())
                        .filter(|value| seen.insert(value.to_bits()))
                        .collect::<Vec<_>>();
                    if values.is_empty() {
                        continue;
                    }
                    for value in &values {
                        index.list.insert(*value, pk.to_owned());
                    }
                    index.by_pk.insert(pk.to_owned(), values);
                }
                AnyIndex::Equality(index) => {
                    let keys = flatten(value.unwrap_or(&Value::Null))
                        .into_iter()
                        .map(scalar_key)
                        .collect::<Vec<_>>();
                    for key in &keys {
                        let values = index.map.entry(key.clone()).or_default();
                        if !values.iter().any(|value| value == pk) {
                            values.push(pk.to_owned());
                        }
                    }
                    index.by_pk.insert(pk.to_owned(), keys);
                }
            }
        }
    }

    pub fn remove(&mut self, pk: &str) {
        for index in self.indexes.values_mut() {
            match index {
                AnyIndex::Range(index) => {
                    if let Some(values) = index.by_pk.remove(pk) {
                        for value in values {
                            index.list.delete(&value, &pk.to_owned());
                        }
                    }
                }
                AnyIndex::Equality(index) => {
                    if let Some(keys) = index.by_pk.remove(pk) {
                        for key in keys {
                            if let Some(values) = index.map.get_mut(&key) {
                                values.retain(|value| value != pk);
                                if values.is_empty() {
                                    index.map.remove(&key);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn find_eq(&self, name: &str, value: &Value) -> Result<Vec<String>, IndexError> {
        match self
            .indexes
            .get(name)
            .ok_or_else(|| IndexError::NotFound(name.into()))?
        {
            AnyIndex::Equality(index) => Ok(index
                .map
                .get(&scalar_key(value))
                .cloned()
                .unwrap_or_default()),
            AnyIndex::Range(_) => Err(IndexError::NotEquality(name.into())),
        }
    }

    pub fn has_eq(&self, name: &str, value: &Value, pk: &str) -> Result<bool, IndexError> {
        Ok(self.find_eq(name, value)?.iter().any(|value| value == pk))
    }

    pub fn find_range(
        &self,
        name: &str,
        options: &NumericRangeOptions,
    ) -> Result<Vec<NumericRangeEntry>, IndexError> {
        let AnyIndex::Range(index) = self
            .indexes
            .get(name)
            .ok_or_else(|| IndexError::NotFound(name.into()))?
        else {
            return Err(IndexError::NotRange(name.into()));
        };
        let range = RangeOptions {
            gte: (!options.min_exclusive).then_some(options.min).flatten(),
            gt: options.min_exclusive.then_some(options.min).flatten(),
            lte: (!options.max_exclusive).then_some(options.max).flatten(),
            lt: options.max_exclusive.then_some(options.max).flatten(),
            offset: options.offset,
            count: options.count,
            reverse: options.reverse,
        };
        Ok(index
            .list
            .range(&range)
            .into_iter()
            .map(|entry| NumericRangeEntry {
                pk: entry.value,
                value: entry.key,
            })
            .collect())
    }

    pub fn rebuild<'a>(&mut self, entries: impl IntoIterator<Item = (&'a str, &'a Value)>) {
        for index in self.indexes.values_mut() {
            match index {
                AnyIndex::Range(index) => {
                    index.list = SkipList::with_comparators(compare_number, compare_string);
                    index.by_pk.clear();
                }
                AnyIndex::Equality(index) => {
                    index.map.clear();
                    index.by_pk.clear();
                }
            }
        }
        for (key, value) in entries {
            if value.is_object() {
                self.add(key, value);
            }
        }
    }
}

fn flatten(value: &Value) -> Vec<&Value> {
    value
        .as_array()
        .map_or_else(|| vec![value], |values| values.iter().collect())
}

fn stable_stringify(value: &Value) -> String {
    match value {
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(stable_stringify)
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
                        stable_stringify(&map[key])
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        _ => serde_json::to_string(value).unwrap(),
    }
}

fn scalar_key(value: &Value) -> String {
    match value {
        Value::String(value) => format!("string:{value}"),
        Value::Number(value) => format!("number:{value}"),
        Value::Bool(value) => format!("boolean:{value}"),
        _ => format!("json:{}", stable_stringify(value)),
    }
}

fn unique_error(index: &str, value: &Value) -> IndexError {
    IndexError::UniqueViolation {
        index: index.into(),
        value: serde_json::to_string(value).unwrap(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maintains_equality_range_and_unique_indexes() {
        let mut indexes = IndexManager::default();
        indexes
            .create(
                "email",
                IndexDef {
                    field: "email".into(),
                    index_type: IndexType::Equality,
                    unique: true,
                    sparse: true,
                },
            )
            .unwrap();
        indexes
            .create(
                "score",
                IndexDef {
                    field: "score".into(),
                    index_type: IndexType::Range,
                    unique: false,
                    sparse: true,
                },
            )
            .unwrap();
        indexes.add("a", &serde_json::json!({"email":"a@x", "score":2}));
        indexes.add("b", &serde_json::json!({"email":"b@x", "score":[1, 1, 3]}));
        assert_eq!(
            indexes.find_eq("email", &Value::from("a@x")).unwrap(),
            vec!["a"]
        );
        assert_eq!(
            indexes
                .find_range("score", &NumericRangeOptions::default())
                .unwrap()
                .iter()
                .map(|entry| entry.pk.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "a", "b"]
        );
        assert!(matches!(
            indexes.check_unique("c", &serde_json::json!({"email":"a@x"})),
            Err(IndexError::UniqueViolation { .. })
        ));
        indexes.remove("a");
        indexes
            .check_unique("c", &serde_json::json!({"email":"a@x"}))
            .unwrap();
    }

    #[test]
    fn canonicalizes_object_keys_and_allows_unique_batch_swap() {
        let mut indexes = IndexManager::default();
        indexes
            .create(
                "value",
                IndexDef {
                    field: "v".into(),
                    index_type: IndexType::Equality,
                    unique: true,
                    sparse: true,
                },
            )
            .unwrap();
        indexes.add("a", &serde_json::json!({"v":{"a":1,"b":2}}));
        assert_eq!(
            indexes
                .find_eq("value", &serde_json::json!({"b":2,"a":1}))
                .unwrap(),
            vec!["a"]
        );
        indexes.add("b", &serde_json::json!({"v":"b"}));
        indexes
            .check_unique_batch(&[
                BatchIndexOp {
                    pk: "a".into(),
                    op: BatchIndexOpType::Set,
                    doc: serde_json::json!({"v":"b"}),
                },
                BatchIndexOp {
                    pk: "b".into(),
                    op: BatchIndexOpType::Set,
                    doc: serde_json::json!({"v":{"a":1,"b":2}}),
                },
            ])
            .unwrap();
    }
}
