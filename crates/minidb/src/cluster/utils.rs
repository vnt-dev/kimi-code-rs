use std::{fmt, time::Duration};

pub const CLUSTER_META_FILE: &str = "cluster.meta.json";
pub const CLUSTER_INDEX_FILE: &str = "cluster.indexes.json";
const SHARD_DIR_PREFIX: &str = "shard-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidShardCount(pub usize);

impl fmt::Display for InvalidShardCount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "shard_count must be a positive integer, got {}",
            self.0
        )
    }
}

impl std::error::Error for InvalidShardCount {}

// Original: packages/minidb/src/cluster/utils.ts, shardDirName().
pub fn shard_dir_name(shard_id: usize, shard_count: usize) -> String {
    let width = 2.max(shard_count.saturating_sub(1).to_string().len());
    format!("{SHARD_DIR_PREFIX}{shard_id:0width$}")
}

// Original: packages/minidb/src/cluster/utils.ts, stableHash32().
pub fn stable_hash32(key: &str, seed: u32) -> u32 {
    let data = key.as_bytes();
    let mut hash = seed;
    const C1: u32 = 0xcc9e_2d51;
    const C2: u32 = 0x1b87_3593;

    let mut chunks = data.chunks_exact(4);
    for chunk in &mut chunks {
        let mut value = u32::from_le_bytes(chunk.try_into().expect("four-byte chunk"));
        value = value.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2);
        hash ^= value;
        hash = hash
            .rotate_left(13)
            .wrapping_mul(5)
            .wrapping_add(0xe654_6b64);
    }

    let remainder = chunks.remainder();
    let mut tail = 0_u32;
    if remainder.len() == 3 {
        tail ^= u32::from(remainder[2]) << 16;
    }
    if remainder.len() >= 2 {
        tail ^= u32::from(remainder[1]) << 8;
    }
    if !remainder.is_empty() {
        tail ^= u32::from(remainder[0]);
        tail = tail.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2);
        hash ^= tail;
    }

    hash ^= data.len() as u32;
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x85eb_ca6b);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(0xc2b2_ae35);
    hash ^ (hash >> 16)
}

// Original: packages/minidb/src/cluster/utils.ts, shardFor().
pub fn shard_for(key: &str, shard_count: usize) -> Result<usize, InvalidShardCount> {
    if shard_count == 0 {
        return Err(InvalidShardCount(shard_count));
    }
    Ok(stable_hash32(key, 0) as usize % shard_count)
}

pub async fn sleep(duration: Duration) {
    tokio::time::sleep(duration).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming_hashing_and_routing_are_stable() {
        assert_eq!(shard_dir_name(3, 100), "shard-03");
        assert_eq!(shard_dir_name(3, 1001), "shard-0003");
        assert_eq!(stable_hash32("", 0), 0);
        assert_eq!(stable_hash32("hello", 0), 613_153_351);
        assert_eq!(shard_for("dist:42", 16), shard_for("dist:42", 16));
        assert_eq!(shard_for("key", 0), Err(InvalidShardCount(0)));
    }
}
