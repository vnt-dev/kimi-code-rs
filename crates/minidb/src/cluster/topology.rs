use std::{
    io,
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{minidb::CodecName, wal::FsyncPolicy};

use super::{
    types::ClusterMeta,
    utils::{CLUSTER_META_FILE, shard_dir_name},
};

const META_VERSION: u32 = 1;
const DEFAULT_SHARD_COUNT: usize = 16;

#[derive(Debug, Error)]
pub enum TopologyError {
    #[error("shard_count must be a positive integer, got 0")]
    InvalidShardCount,
    #[error("unsupported cluster meta version {0}")]
    UnsupportedVersion(u32),
    #[error("cluster was created with shard_count={actual}, got {requested}")]
    ShardCountMismatch { actual: usize, requested: usize },
    #[error("cluster was created with value codec {actual:?}, got {requested:?}")]
    CodecMismatch {
        actual: CodecName,
        requested: CodecName,
    },
    #[error("cluster was created with fsync policy {actual:?}, got {requested:?}")]
    FsyncMismatch {
        actual: FsyncPolicy,
        requested: FsyncPolicy,
    },
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct Topology {
    pub directory: PathBuf,
    pub meta: ClusterMeta,
}

impl Topology {
    // Original: packages/minidb/src/cluster/topology.ts, Topology.open().
    pub async fn open(
        directory: impl Into<PathBuf>,
        shard_count: Option<usize>,
        codec: CodecName,
        fsync_policy: FsyncPolicy,
    ) -> Result<Self, TopologyError> {
        if shard_count == Some(0) {
            return Err(TopologyError::InvalidShardCount);
        }
        let directory = directory.into();
        tokio::fs::create_dir_all(&directory).await?;
        let path = directory.join(CLUSTER_META_FILE);
        let requested = ClusterMeta {
            version: META_VERSION,
            shard_count: shard_count.unwrap_or(DEFAULT_SHARD_COUNT),
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            value_codec: codec,
            fsync_policy,
        };
        let temporary = directory.join(format!("{CLUSTER_META_FILE}.tmp-{}", std::process::id()));
        tokio::fs::write(&temporary, serde_json::to_vec_pretty(&requested)?).await?;
        match tokio::fs::hard_link(&temporary, &path).await {
            Ok(()) => {
                let _ = tokio::fs::remove_file(&temporary).await;
                return Ok(Self {
                    directory,
                    meta: requested,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                let _ = tokio::fs::remove_file(&temporary).await;
                return Err(error.into());
            }
        }
        let _ = tokio::fs::remove_file(&temporary).await;
        let loaded: ClusterMeta = serde_json::from_slice(&tokio::fs::read(&path).await?)?;
        if loaded.version != META_VERSION {
            return Err(TopologyError::UnsupportedVersion(loaded.version));
        }
        if loaded.shard_count == 0 {
            return Err(TopologyError::InvalidShardCount);
        }
        if let Some(requested) = shard_count
            && requested != loaded.shard_count
        {
            return Err(TopologyError::ShardCountMismatch {
                actual: loaded.shard_count,
                requested,
            });
        }
        if loaded.value_codec != codec {
            return Err(TopologyError::CodecMismatch {
                actual: loaded.value_codec,
                requested: codec,
            });
        }
        if loaded.fsync_policy != fsync_policy {
            return Err(TopologyError::FsyncMismatch {
                actual: loaded.fsync_policy,
                requested: fsync_policy,
            });
        }
        Ok(Self {
            directory,
            meta: loaded,
        })
    }

    pub fn shard_count(&self) -> usize {
        self.meta.shard_count
    }
    pub fn shard_directory(&self, shard_id: usize) -> PathBuf {
        self.directory
            .join(shard_dir_name(shard_id, self.meta.shard_count))
    }
    pub fn all_shard_directories(&self) -> Vec<PathBuf> {
        (0..self.meta.shard_count)
            .map(|id| self.shard_directory(id))
            .collect()
    }
    pub async fn ensure_shard_directories(&self) -> Result<(), io::Error> {
        for directory in self.all_shard_directories() {
            tokio::fs::create_dir_all(directory).await?;
        }
        Ok(())
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}
