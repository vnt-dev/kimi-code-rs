use std::path::{Path, PathBuf};

use super::{
    types::ClusterMeta,
    utils::{InvalidShardCount, shard_dir_name, shard_for},
};

#[derive(Debug, Clone)]
pub struct Router {
    base_directory: PathBuf,
    meta: ClusterMeta,
}

impl Router {
    pub fn new(base_directory: impl Into<PathBuf>, meta: ClusterMeta) -> Self {
        Self {
            base_directory: base_directory.into(),
            meta,
        }
    }
    pub fn shard_count(&self) -> usize {
        self.meta.shard_count
    }
    pub fn shard_for(&self, key: &str) -> Result<usize, InvalidShardCount> {
        shard_for(key, self.meta.shard_count)
    }
    pub fn shard_directory(&self, shard_id: usize) -> PathBuf {
        self.base_directory
            .join(shard_dir_name(shard_id, self.meta.shard_count))
    }
    pub fn shard_ids(&self) -> std::ops::Range<usize> {
        0..self.meta.shard_count
    }
    pub fn base_directory(&self) -> &Path {
        &self.base_directory
    }
}
