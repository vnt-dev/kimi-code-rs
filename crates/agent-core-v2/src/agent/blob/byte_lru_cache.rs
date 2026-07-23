use std::sync::Arc;

use indexmap::IndexMap;

/// Agent-local LRU cache whose capacity is measured in payload bytes.
///
/// Original: `packages/agent-core-v2/src/agent/blob/byteLruCache.ts`.
pub(crate) struct ByteLruCache {
    map: IndexMap<String, Arc<[u8]>>,
    current_bytes: usize,
    max_bytes: usize,
}

impl ByteLruCache {
    pub(crate) fn new(max_bytes: usize) -> Self {
        Self {
            map: IndexMap::new(),
            current_bytes: 0,
            max_bytes,
        }
    }

    // Original: ByteLruCache.get(). `shift_remove` followed by insertion
    // reproduces JavaScript Map's delete/set recency refresh.
    pub(crate) fn get(&mut self, key: &str) -> Option<Arc<[u8]>> {
        let value = self.map.shift_remove(key)?;
        self.map.insert(key.to_owned(), Arc::clone(&value));
        Some(value)
    }

    // Original: ByteLruCache.set(). Replacing an existing entry deliberately
    // skips eviction of other entries, including the source's temporary
    // over-capacity behavior for a larger replacement.
    pub(crate) fn set(&mut self, key: String, value: Arc<[u8]>) {
        let size = value.len();
        let existing = self.map.shift_remove(&key);

        if size > self.max_bytes {
            if let Some(existing) = existing {
                self.current_bytes -= existing.len();
            }
            return;
        }

        if let Some(existing) = existing {
            self.current_bytes -= existing.len();
        } else {
            while !self.map.is_empty() && self.current_bytes.saturating_add(size) > self.max_bytes {
                self.evict_oldest();
            }
        }

        self.current_bytes += size;
        self.map.insert(key, value);
    }

    // Original: ByteLruCache.evictOldest().
    fn evict_oldest(&mut self) {
        let Some((_, value)) = self.map.shift_remove_index(0) else {
            return;
        };
        self.current_bytes -= value.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(value: &[u8]) -> Arc<[u8]> {
        Arc::from(value)
    }

    #[test]
    fn hit_refreshes_recency_before_next_eviction() {
        let mut cache = ByteLruCache::new(6);
        cache.set("a".into(), bytes(b"aa"));
        cache.set("b".into(), bytes(b"bb"));
        cache.set("c".into(), bytes(b"cc"));

        assert_eq!(&*cache.get("a").unwrap(), b"aa");
        cache.set("d".into(), bytes(b"dd"));

        assert!(cache.get("b").is_none());
        assert_eq!(&*cache.get("c").unwrap(), b"cc");
        assert_eq!(&*cache.get("a").unwrap(), b"aa");
        assert_eq!(&*cache.get("d").unwrap(), b"dd");
    }

    #[test]
    fn insertion_evicts_oldest_entries_until_payload_fits() {
        let mut cache = ByteLruCache::new(7);
        cache.set("a".into(), bytes(b"aa"));
        cache.set("b".into(), bytes(b"bb"));
        cache.set("c".into(), bytes(b"cc"));
        cache.set("large".into(), bytes(b"12345"));

        assert!(cache.get("a").is_none());
        assert!(cache.get("b").is_none());
        assert_eq!(&*cache.get("c").unwrap(), b"cc");
        assert_eq!(&*cache.get("large").unwrap(), b"12345");
        assert_eq!(cache.current_bytes, 7);
    }

    #[test]
    fn oversized_payload_is_not_cached_and_removes_same_key_only() {
        let mut cache = ByteLruCache::new(4);
        cache.set("keep".into(), bytes(b"ok"));
        cache.set("replace".into(), bytes(b"xx"));
        cache.set("replace".into(), bytes(b"12345"));
        cache.set("new".into(), bytes(b"12345"));

        assert_eq!(&*cache.get("keep").unwrap(), b"ok");
        assert!(cache.get("replace").is_none());
        assert!(cache.get("new").is_none());
        assert_eq!(cache.current_bytes, 2);
    }

    #[test]
    fn replacement_moves_to_mru_without_evicting_other_entries() {
        let mut cache = ByteLruCache::new(6);
        cache.set("a".into(), bytes(b"aa"));
        cache.set("b".into(), bytes(b"bb"));
        cache.set("a".into(), bytes(b"12345"));

        // This over-capacity state is observable source behavior: replacement
        // does not enter the eviction loop used by a new key.
        assert_eq!(cache.current_bytes, 7);
        assert_eq!(&*cache.get("b").unwrap(), b"bb");
        assert_eq!(&*cache.get("a").unwrap(), b"12345");
    }

    #[test]
    fn zero_capacity_still_caches_an_empty_buffer() {
        let mut cache = ByteLruCache::new(0);
        cache.set("empty".into(), bytes(b""));
        cache.set("nonempty".into(), bytes(b"x"));
        assert_eq!(&*cache.get("empty").unwrap(), b"");
        assert!(cache.get("nonempty").is_none());
    }

    #[test]
    fn zero_byte_insertion_repairs_over_capacity_replacement_state() {
        let mut cache = ByteLruCache::new(6);
        cache.set("a".into(), bytes(b"aa"));
        cache.set("b".into(), bytes(b"bb"));
        cache.set("a".into(), bytes(b"12345"));
        assert_eq!(cache.current_bytes, 7);

        cache.set("empty".into(), bytes(b""));
        assert!(cache.get("b").is_none());
        assert_eq!(&*cache.get("a").unwrap(), b"12345");
        assert_eq!(&*cache.get("empty").unwrap(), b"");
        assert_eq!(cache.current_bytes, 5);
    }
}
