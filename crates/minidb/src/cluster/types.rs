use serde::{Deserialize, Serialize};

use crate::{
    compound_index::CompoundIndexDef,
    index_manager::IndexDef,
    minidb::{CodecName, OpenOptions},
    wal::FsyncPolicy,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CrossShardMode {
    #[default]
    BestEffort,
    TwoPhaseCommit,
    None,
}

pub struct ClusterOpenOptions<V> {
    pub base: OpenOptions<V>,
    pub shard_count: Option<usize>,
    pub read_only: bool,
    pub lock_renew_millis: u64,
    pub lock_acquire_timeout_millis: u64,
    pub lock_hold_millis: u64,
    pub max_writers: usize,
    pub max_readers: Option<usize>,
    pub cross_shard: CrossShardMode,
}

impl<V> ClusterOpenOptions<V> {
    pub fn new(base: OpenOptions<V>) -> Self {
        Self {
            base,
            shard_count: None,
            read_only: false,
            lock_renew_millis: 10_000,
            lock_acquire_timeout_millis: 30_000,
            lock_hold_millis: 250,
            max_writers: 16,
            max_readers: None,
            cross_shard: CrossShardMode::BestEffort,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterMeta {
    pub version: u32,
    pub shard_count: usize,
    pub created_at: String,
    pub value_codec: CodecName,
    pub fsync_policy: FsyncPolicy,
}

#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    pub gte: Option<String>,
    pub gt: Option<String>,
    pub lte: Option<String>,
    pub lt: Option<String>,
    pub prefix: Option<String>,
    pub limit: Option<usize>,
    pub reverse: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterIndexRegistry {
    #[serde(default)]
    pub indexes: Vec<NamedIndexDefinition>,
    #[serde(default)]
    pub compound_indexes: Vec<NamedCompoundDefinition>,
    #[serde(default)]
    pub text_indexes: Vec<NamedTextDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedIndexDefinition {
    pub name: String,
    pub def: IndexDef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedCompoundDefinition {
    pub name: String,
    pub def: CompoundIndexDef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedTextDefinition {
    pub name: String,
    pub fields: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompactResult {
    pub compacted: Vec<usize>,
    pub skipped: Vec<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClusterStats {
    pub shard_count: usize,
    pub writers_cached: usize,
    pub readers_cached: usize,
    pub writer_opens: u64,
    pub reader_opens: u64,
    pub reader_reopens: u64,
    pub incremental_catchups: u64,
    pub catchup_frames_applied: u64,
    pub lock_waits: u64,
    pub evictions: u64,
}
