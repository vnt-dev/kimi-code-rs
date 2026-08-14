use std::{
    cmp::Reverse,
    collections::{BTreeMap, BinaryHeap, HashMap},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;

use crate::skiplist::{RangeOptions, SkipList};

const DISK_REF_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueFile {
    Snapshot,
    Wal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueLoc {
    pub file: ValueFile,
    pub offset: u64,
    pub len: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueRef {
    Memory(Vec<u8>),
    Disk(ValueLoc),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoreRecord {
    pub value_ref: ValueRef,
    pub expire_at: i64,
    pub sequence: u64,
    pub datetimes: Option<BTreeMap<String, i64>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoreEntry {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub expire_at: i64,
    pub datetimes: Option<BTreeMap<String, i64>>,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("Store cannot read disk-backed value without a ValueReader")]
    MissingValueReader,
    #[error("failed to read disk-backed value: {0}")]
    Read(String),
}

pub type ValueReader = Arc<dyn Fn(ValueLoc) -> Result<Vec<u8>, StoreError> + Send + Sync>;

#[derive(Default)]
pub struct StoreOptions {
    pub read_value: Option<ValueReader>,
}

pub struct Store {
    records: HashMap<Vec<u8>, StoreRecord>,
    order: SkipList<Vec<u8>, Vec<u8>>,
    expirations: BinaryHeap<Reverse<(i64, u64, Vec<u8>)>>,
    expired: Vec<(Vec<u8>, StoreRecord)>,
    sequence: u64,
    bytes: usize,
    expiring: usize,
    read_value: Option<ValueReader>,
}

impl Store {
    pub fn new(options: StoreOptions) -> Self {
        Self {
            records: HashMap::new(),
            order: SkipList::with_comparators(Vec::<u8>::cmp, Vec::<u8>::cmp),
            expirations: BinaryHeap::new(),
            expired: Vec::new(),
            sequence: 0,
            bytes: 0,
            expiring: 0,
            read_value: options.read_value,
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn map(&self) -> &HashMap<Vec<u8>, StoreRecord> {
        &self.records
    }

    pub fn len(&self) -> usize {
        if self.expiring == 0 {
            return self.records.len();
        }
        let now = now_millis();
        self.records
            .values()
            .filter(|record| record.expire_at == 0 || record.expire_at > now)
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn metadata_bytes(datetimes: Option<&BTreeMap<String, i64>>) -> usize {
        datetimes.map_or(0, |datetimes| {
            serde_json::to_vec(&serde_json::json!({ "dt": datetimes }))
                .map_or(0, |bytes| bytes.len())
        })
    }

    fn value_ref_bytes(value_ref: &ValueRef) -> usize {
        match value_ref {
            ValueRef::Memory(value) => value.len(),
            ValueRef::Disk(_) => DISK_REF_BYTES,
        }
    }

    fn record_size(key: &[u8], record: &StoreRecord) -> usize {
        key.len()
            + Self::value_ref_bytes(&record.value_ref)
            + Self::metadata_bytes(record.datetimes.as_ref())
    }

    pub fn record_bytes(&self, key: &[u8]) -> usize {
        self.records
            .get(key)
            .map_or(0, |record| Self::record_size(key, record))
    }

    pub fn estimate_set_bytes(
        &self,
        key: &[u8],
        value: &[u8],
        datetimes: Option<&BTreeMap<String, i64>>,
        count_value: bool,
    ) -> usize {
        key.len()
            + if count_value {
                value.len()
            } else {
                DISK_REF_BYTES
            }
            + Self::metadata_bytes(datetimes)
    }

    // Original: packages/minidb/src/store.ts, Store.set().
    pub fn set(
        &mut self,
        key: impl Into<Vec<u8>>,
        value: impl Into<Vec<u8>>,
        expire_at: i64,
        datetimes: Option<BTreeMap<String, i64>>,
    ) {
        self.set_ref(key, ValueRef::Memory(value.into()), expire_at, datetimes);
    }

    // Original: packages/minidb/src/store.ts, Store.setRef().
    pub fn set_ref(
        &mut self,
        key: impl Into<Vec<u8>>,
        value_ref: ValueRef,
        expire_at: i64,
        datetimes: Option<BTreeMap<String, i64>>,
    ) {
        let key = key.into();
        let previous = self.records.remove(&key);
        if let Some(previous) = &previous {
            self.bytes = self.bytes.saturating_sub(Self::record_size(&key, previous));
            if previous.expire_at != 0 {
                self.expiring = self.expiring.saturating_sub(1);
            }
        }
        self.sequence = self.sequence.wrapping_add(1);
        let record = StoreRecord {
            value_ref,
            expire_at,
            sequence: self.sequence,
            datetimes,
        };
        self.bytes += Self::record_size(&key, &record);
        if previous.is_none() {
            self.order.insert(key.clone(), key.clone());
        }
        if expire_at != 0 {
            self.expiring += 1;
            self.expirations
                .push(Reverse((expire_at, self.sequence, key.clone())));
            if self.expirations.len() > self.records.len() * 2 + 64 {
                self.rebuild_expirations();
            }
        }
        self.records.insert(key, record);
    }

    fn rebuild_expirations(&mut self) {
        self.expirations.clear();
        for (key, record) in &self.records {
            if record.expire_at != 0 {
                self.expirations
                    .push(Reverse((record.expire_at, record.sequence, key.clone())));
            }
        }
    }

    fn remove(&mut self, key: &[u8], expired: bool) -> Option<StoreRecord> {
        let record = self.records.remove(key)?;
        self.bytes = self.bytes.saturating_sub(Self::record_size(key, &record));
        if record.expire_at != 0 {
            self.expiring = self.expiring.saturating_sub(1);
        }
        self.order.delete(&key.to_vec(), &key.to_vec());
        if expired {
            self.expired.push((key.to_vec(), record.clone()));
        }
        Some(record)
    }

    fn expire_if_needed(&mut self, key: &[u8], now: i64) -> bool {
        let expired = self
            .records
            .get(key)
            .is_some_and(|record| record.expire_at != 0 && record.expire_at <= now);
        if expired {
            self.remove(key, true);
        }
        expired
    }

    fn materialize(&self, value_ref: &ValueRef) -> Result<Vec<u8>, StoreError> {
        match value_ref {
            ValueRef::Memory(value) => Ok(value.clone()),
            ValueRef::Disk(location) => {
                self.read_value
                    .as_ref()
                    .ok_or(StoreError::MissingValueReader)?(*location)
            }
        }
    }

    // Original: packages/minidb/src/store.ts, Store.get().
    pub fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        if self.expire_if_needed(key, now_millis()) {
            return Ok(None);
        }
        self.records
            .get(key)
            .map(|record| self.materialize(&record.value_ref))
            .transpose()
    }

    pub fn get_record(&mut self, key: &[u8]) -> Option<&StoreRecord> {
        if self.expire_if_needed(key, now_millis()) {
            return None;
        }
        self.records.get(key)
    }

    pub fn del(&mut self, key: &[u8]) -> bool {
        self.remove(key, false).is_some()
    }

    pub fn has(&mut self, key: &[u8]) -> bool {
        !self.expire_if_needed(key, now_millis()) && self.records.contains_key(key)
    }

    pub fn entries(&self) -> Result<Vec<StoreEntry>, StoreError> {
        let now = now_millis();
        self.records
            .iter()
            .filter(|(_, record)| record.expire_at == 0 || record.expire_at > now)
            .map(|(key, record)| {
                Ok(StoreEntry {
                    key: key.clone(),
                    value: self.materialize(&record.value_ref)?,
                    expire_at: record.expire_at,
                    datetimes: record.datetimes.clone(),
                })
            })
            .collect()
    }

    pub fn scan(&mut self, options: &RangeOptions<Vec<u8>>) -> Result<Vec<StoreEntry>, StoreError> {
        let keys = self
            .order
            .range(options)
            .into_iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>();
        let mut output = Vec::new();
        for key in keys {
            if self.expire_if_needed(&key, now_millis()) {
                continue;
            }
            if let Some(record) = self.records.get(&key) {
                output.push(StoreEntry {
                    key,
                    value: self.materialize(&record.value_ref)?,
                    expire_at: record.expire_at,
                    datetimes: record.datetimes.clone(),
                });
            }
        }
        Ok(output)
    }

    pub fn prefix(&mut self, prefix: &[u8], limit: usize) -> Result<Vec<StoreEntry>, StoreError> {
        let mut upper = prefix.to_vec();
        upper.extend_from_slice("\u{ffff}".as_bytes());
        self.scan(&RangeOptions {
            gte: Some(prefix.to_vec()),
            lt: Some(upper),
            count: Some(limit),
            ..RangeOptions::default()
        })
    }

    pub fn raw_keys(&mut self, options: &RangeOptions<Vec<u8>>) -> Vec<Vec<u8>> {
        let keys = self
            .order
            .range(options)
            .into_iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>();
        keys.into_iter()
            .filter(|key| !self.expire_if_needed(key, now_millis()))
            .collect()
    }

    pub fn remap_locations(
        &mut self,
        mut remap: impl FnMut(&[u8], ValueLoc, &StoreRecord) -> Option<ValueLoc>,
    ) {
        for (key, record) in &mut self.records {
            let ValueRef::Disk(location) = record.value_ref else {
                continue;
            };
            if let Some(location) = remap(key, location, record) {
                record.value_ref = ValueRef::Disk(location);
            }
        }
    }

    // Original: packages/minidb/src/store.ts, Store.reapExpired()/activeExpire().
    pub fn reap_expired(&mut self, max: Option<usize>) -> usize {
        let now = now_millis();
        let mut reaped = 0;
        while max.is_none_or(|max| reaped < max) {
            let Some(Reverse((expires, sequence, key))) = self.expirations.peek().cloned() else {
                break;
            };
            if expires > now {
                break;
            }
            self.expirations.pop();
            let current = self.records.get(&key).is_some_and(|record| {
                record.sequence == sequence && record.expire_at != 0 && record.expire_at <= now
            });
            if current && self.remove(&key, true).is_some() {
                reaped += 1;
            }
        }
        reaped
    }

    pub fn take_expired(&mut self) -> Vec<(Vec<u8>, StoreRecord)> {
        std::mem::take(&mut self.expired)
    }
}

pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_orders_expires_and_tracks_memory() {
        let mut store = Store::new(StoreOptions::default());
        store.set(b"b".to_vec(), b"2".to_vec(), 0, None);
        store.set(b"a".to_vec(), b"1".to_vec(), 0, None);
        assert_eq!(store.get(b"a").unwrap(), Some(b"1".to_vec()));
        assert_eq!(
            store
                .scan(&RangeOptions::default())
                .unwrap()
                .into_iter()
                .map(|entry| entry.key)
                .collect::<Vec<_>>(),
            vec![b"a".to_vec(), b"b".to_vec()]
        );
        store.set(b"expired".to_vec(), b"x".to_vec(), now_millis() - 1, None);
        assert_eq!(store.get(b"expired").unwrap(), None);
        assert_eq!(store.take_expired().len(), 1);
        assert!(store.bytes() > 0);
    }
}
