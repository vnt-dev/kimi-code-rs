use std::{collections::HashMap, io, path::Path};

use thiserror::Error;
use tokio::{fs::File, io::AsyncWriteExt};

use crate::{
    codec::{CodecError, Frame, HEADER_SIZE, TYPE_SET, encode_frame},
    store::{Store, StoreError, ValueFile, ValueLoc},
};

const FLUSH_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotResult {
    pub count: usize,
    pub bytes: u64,
    pub locations: HashMap<Vec<u8>, ValueLoc>,
}

#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("failed to encode snapshot metadata: {0}")]
    Metadata(#[from] serde_json::Error),
}

// Original: packages/minidb/src/snapshot.ts, writeSnapshot().
pub async fn write_snapshot(
    store: &Store,
    temporary_path: impl AsRef<Path>,
    yield_every: usize,
) -> Result<SnapshotResult, SnapshotError> {
    let yield_every = if yield_every == 0 { 2_000 } else { yield_every };
    let mut file = File::create(temporary_path).await?;
    let mut count = 0;
    let mut bytes = 0_u64;
    let mut batch = Vec::with_capacity(FLUSH_BYTES);
    let mut locations = HashMap::new();

    for entry in store.entries()? {
        let meta = entry
            .datetimes
            .as_ref()
            .map(|datetimes| serde_json::to_vec(&serde_json::json!({ "dt": datetimes })))
            .transpose()?;
        let value_len = entry.value.len() as u32;
        let frame = encode_frame(&Frame {
            frame_type: TYPE_SET,
            key: entry.key.clone(),
            value: entry.value,
            meta,
            expire_at: entry.expire_at,
        })?;
        let frame_offset = bytes + batch.len() as u64;
        locations.insert(
            entry.key.clone(),
            ValueLoc {
                file: ValueFile::Snapshot,
                offset: frame_offset + HEADER_SIZE as u64 + entry.key.len() as u64,
                len: value_len,
            },
        );
        batch.extend_from_slice(&frame);
        count += 1;
        if batch.len() >= FLUSH_BYTES {
            file.write_all(&batch).await?;
            bytes += batch.len() as u64;
            batch.clear();
        }
        if count % yield_every == 0 {
            tokio::task::yield_now().await;
        }
    }
    if !batch.is_empty() {
        file.write_all(&batch).await?;
        bytes += batch.len() as u64;
    }
    file.sync_all().await?;
    Ok(SnapshotResult {
        count,
        bytes,
        locations,
    })
}

#[cfg(test)]
mod tests {
    use crate::{
        codec::{CorruptionMode, scan_frame_refs_file},
        store::StoreOptions,
    };

    use super::*;

    #[tokio::test]
    async fn writes_live_set_frames_and_value_locations() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("snapshot.tmp");
        let mut store = Store::new(StoreOptions::default());
        store.set(b"key".to_vec(), b"value".to_vec(), 0, None);
        let result = write_snapshot(&store, &path, 1).await.unwrap();
        assert_eq!(result.count, 1);
        let scan = scan_frame_refs_file(&path, CorruptionMode::Strict).unwrap();
        assert_eq!(scan.frames.len(), 1);
        assert_eq!(
            result.locations[b"key".as_slice()].offset,
            scan.frames[0].value_offset
        );
        assert_eq!(result.locations[b"key".as_slice()].len, 5);
    }
}
