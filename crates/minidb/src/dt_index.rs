use std::collections::{BTreeMap, HashMap};

use crate::skiplist::{RangeOptions, SkipList, compare_string};

struct DateTimeColumn {
    list: SkipList<i64, String>,
    by_key: HashMap<String, i64>,
}

impl DateTimeColumn {
    fn new() -> Self {
        Self {
            list: SkipList::with_comparators(i64::cmp, compare_string),
            by_key: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DateTimeRangeEntry {
    pub key: String,
    pub value: i64,
}

#[derive(Default)]
pub struct DateTimeIndex {
    columns: HashMap<String, DateTimeColumn>,
    by_key: HashMap<String, BTreeMap<String, i64>>,
}

impl DateTimeIndex {
    // Original: packages/minidb/src/dt-index.ts, DtIndex.set().
    pub fn set(&mut self, key: &str, datetimes: Option<&BTreeMap<String, i64>>) {
        let old = self.by_key.get(key).cloned().unwrap_or_default();
        let next = datetimes.cloned().unwrap_or_default();
        for (column, old_value) in &old {
            if next.get(column) != Some(old_value)
                && let Some(index) = self.columns.get_mut(column)
            {
                index.list.delete(old_value, &key.to_owned());
                index.by_key.remove(key);
                if index.by_key.is_empty() {
                    self.columns.remove(column);
                }
            }
        }
        for (column, value) in &next {
            if old.get(column) == Some(value) {
                continue;
            }
            let index = self
                .columns
                .entry(column.clone())
                .or_insert_with(DateTimeColumn::new);
            index.list.insert(*value, key.to_owned());
            index.by_key.insert(key.to_owned(), *value);
        }
        if next.is_empty() {
            self.by_key.remove(key);
        } else {
            self.by_key.insert(key.to_owned(), next);
        }
    }

    pub fn delete(&mut self, key: &str) {
        let Some(old) = self.by_key.remove(key) else {
            return;
        };
        for (column, value) in old {
            if let Some(index) = self.columns.get_mut(&column) {
                index.list.delete(&value, &key.to_owned());
                index.by_key.remove(key);
                if index.by_key.is_empty() {
                    self.columns.remove(&column);
                }
            }
        }
    }

    pub fn range(&self, column: &str, options: &RangeOptions<i64>) -> Vec<DateTimeRangeEntry> {
        self.columns.get(column).map_or_else(Vec::new, |index| {
            index
                .list
                .range(options)
                .into_iter()
                .map(|entry| DateTimeRangeEntry {
                    key: entry.value,
                    value: entry.key,
                })
                .collect()
        })
    }

    pub fn columns(&self) -> Vec<String> {
        self.columns.keys().cloned().collect()
    }

    pub fn rebuild<'a>(
        &mut self,
        entries: impl IntoIterator<Item = (&'a str, Option<&'a BTreeMap<String, i64>>)>,
    ) {
        self.columns.clear();
        self.by_key.clear();
        for (key, datetimes) in entries {
            self.set(key, datetimes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_and_ranges_datetime_columns() {
        let mut index = DateTimeIndex::default();
        index.set("a", Some(&BTreeMap::from([("created".into(), 1)])));
        index.set("b", Some(&BTreeMap::from([("created".into(), 2)])));
        assert_eq!(
            index.range(
                "created",
                &RangeOptions {
                    gte: Some(1),
                    lte: Some(2),
                    ..RangeOptions::default()
                }
            ),
            vec![
                DateTimeRangeEntry {
                    key: "a".into(),
                    value: 1
                },
                DateTimeRangeEntry {
                    key: "b".into(),
                    value: 2
                }
            ]
        );
        index.delete("a");
        assert_eq!(index.range("created", &RangeOptions::default()).len(), 1);
    }
}
