use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{
    codec::{
        CodecError, CorruptionMode, FrameRef, MAGIC, TYPE_BATCH, TYPE_DEL, TYPE_SET,
        scan_batch_op_refs, scan_frame_refs,
    },
    store::{Store, ValueFile, ValueLoc, ValueRef, now_millis},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ValueMode {
    #[default]
    Memory,
    Disk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryInfo {
    pub snapshot_frames: usize,
    pub wal_frames: usize,
    pub truncated_wal: bool,
    pub corrupt_ranges: Vec<(u64, u64)>,
    pub snapshot_corrupt_ranges: Vec<(u64, u64)>,
    pub lost_bytes: u64,
    pub wal_scan_end: u64,
    pub wal_device: u64,
    pub wal_inode: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalAnchor {
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatchUpResult {
    pub offset: u64,
    pub applied_frames: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecoveredOp {
    Set {
        key: Vec<u8>,
        value_ref: ValueRef,
        expire_at: i64,
        datetimes: Option<BTreeMap<String, i64>>,
    },
    Del {
        key: Vec<u8>,
    },
}

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error("invalid recovery metadata: {0}")]
    Metadata(#[from] serde_json::Error),
    #[error("recovery task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

#[derive(Debug, Clone)]
pub struct RecoveryOptions {
    pub directory: PathBuf,
    pub mode: CorruptionMode,
    pub truncate: bool,
    pub value_mode: ValueMode,
}

impl RecoveryOptions {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            mode: CorruptionMode::Resync,
            truncate: true,
            value_mode: ValueMode::Memory,
        }
    }
}

struct ScannedFile {
    operations: Vec<RecoveredOp>,
    frames: usize,
    corrupt_ranges: Vec<(u64, u64)>,
    scan_end: u64,
    size: u64,
    anchor: WalAnchor,
}

// Original: packages/minidb/src/recovery.ts, frameToOps().
pub fn frame_to_ops(
    frame: &FrameRef,
    file_kind: ValueFile,
    file: &mut File,
    value_mode: ValueMode,
) -> Result<Vec<RecoveredOp>, RecoveryError> {
    match frame.frame_type {
        TYPE_SET => set_ref_to_ops(
            frame.key.clone(),
            frame.value_offset,
            frame.value_len,
            frame.meta.as_deref(),
            frame.expire_at,
            file_kind,
            file,
            value_mode,
        ),
        TYPE_DEL => Ok(vec![RecoveredOp::Del {
            key: frame.key.clone(),
        }]),
        TYPE_BATCH => {
            let body = read_at(file, frame.value_offset, frame.value_len)?;
            let Ok(operations) = scan_batch_op_refs(&body, frame.value_offset) else {
                return Ok(Vec::new());
            };
            let mut output = Vec::new();
            for operation in operations {
                if operation.op_type == TYPE_SET {
                    output.extend(set_ref_to_ops(
                        operation.key,
                        operation.value_offset,
                        operation.value_len,
                        operation.meta.as_deref(),
                        operation.expire_at,
                        file_kind,
                        file,
                        value_mode,
                    )?);
                } else if operation.op_type == TYPE_DEL {
                    output.push(RecoveredOp::Del { key: operation.key });
                }
            }
            Ok(output)
        }
        _ => Ok(Vec::new()),
    }
}

#[allow(clippy::too_many_arguments)]
fn set_ref_to_ops(
    key: Vec<u8>,
    value_offset: u64,
    value_len: u32,
    meta: Option<&[u8]>,
    expire_at: i64,
    file_kind: ValueFile,
    file: &mut File,
    value_mode: ValueMode,
) -> Result<Vec<RecoveredOp>, RecoveryError> {
    if expire_at != 0 && expire_at <= now_millis() {
        return Ok(vec![RecoveredOp::Del { key }]);
    }
    let datetimes = parse_meta(meta)?;
    let value_ref = match value_mode {
        ValueMode::Memory => ValueRef::Memory(read_at(file, value_offset, value_len)?),
        ValueMode::Disk => ValueRef::Disk(ValueLoc {
            file: file_kind,
            offset: value_offset,
            len: value_len,
        }),
    };
    Ok(vec![RecoveredOp::Set {
        key,
        value_ref,
        expire_at,
        datetimes,
    }])
}

fn parse_meta(meta: Option<&[u8]>) -> Result<Option<BTreeMap<String, i64>>, serde_json::Error> {
    #[derive(serde::Deserialize)]
    struct Metadata {
        #[serde(default, deserialize_with = "deserialize_datetimes")]
        dt: Option<BTreeMap<String, i64>>,
    }
    meta.map(|bytes| serde_json::from_slice::<Metadata>(bytes).map(|meta| meta.dt))
        .transpose()
        .map(Option::flatten)
}

// Legacy writers stored datetime metadata as JSON numbers (possibly `100.0`);
// accept both integers and integer-valued floats, truncating any fraction.
fn deserialize_datetimes<'de, D>(deserializer: D) -> Result<Option<BTreeMap<String, i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct DatetimesVisitor;

    impl<'de> serde::de::Visitor<'de> for DatetimesVisitor {
        type Value = Option<BTreeMap<String, i64>>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                formatter,
                "a map of datetime column names to integer milliseconds"
            )
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut output = BTreeMap::new();
            while let Some(key) = map.next_key::<String>()? {
                let value: serde_json::Value = map.next_value()?;
                let Some(number) = value.as_f64().filter(|number| number.is_finite()) else {
                    return Err(serde::de::Error::custom(format!(
                        "datetime value for column {key:?} must be a finite number"
                    )));
                };
                let truncated = number.trunc();
                if truncated < i64::MIN as f64 || truncated >= i64::MAX as f64 {
                    return Err(serde::de::Error::custom(format!(
                        "datetime value for column {key:?} is out of range"
                    )));
                }
                output.insert(key, truncated as i64);
            }
            Ok(Some(output))
        }
    }

    deserializer.deserialize_any(DatetimesVisitor)
}

fn read_at(file: &mut File, offset: u64, len: u32) -> io::Result<Vec<u8>> {
    if len == 0 {
        return Ok(Vec::new());
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut output = vec![0; len as usize];
    file.read_exact(&mut output)?;
    Ok(output)
}

fn scan_file(
    path: &Path,
    file_kind: ValueFile,
    mode: CorruptionMode,
    value_mode: ValueMode,
) -> Result<Option<ScannedFile>, RecoveryError> {
    let mut file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    let size = metadata.len();
    let anchor = metadata_anchor(path, &metadata)?;
    let scan = scan_frame_refs(&mut file, mode, 0)?;
    let mut operations = Vec::new();
    for frame in &scan.frames {
        operations.extend(frame_to_ops(frame, file_kind, &mut file, value_mode)?);
    }
    Ok(Some(ScannedFile {
        operations,
        frames: scan.frames.len(),
        corrupt_ranges: scan.corrupt_ranges,
        scan_end: scan.eof_offset,
        size,
        anchor,
    }))
}

// Original: packages/minidb/src/recovery.ts, recover(). Blocking scans run outside Tokio worker threads.
pub async fn recover(
    store: &mut Store,
    options: RecoveryOptions,
) -> Result<RecoveryInfo, RecoveryError> {
    let directory = options.directory.clone();
    let (snapshot, wal) = tokio::task::spawn_blocking(move || {
        let snapshot = scan_file(
            &directory.join("db.snapshot"),
            ValueFile::Snapshot,
            options.mode,
            options.value_mode,
        )?;
        let wal = scan_file(
            &directory.join("db.wal"),
            ValueFile::Wal,
            options.mode,
            options.value_mode,
        )?;
        Ok::<_, RecoveryError>((snapshot, wal))
    })
    .await??;
    if let Some(snapshot) = &snapshot {
        apply_operations(store, snapshot.operations.clone());
    }
    if let Some(wal) = &wal {
        apply_operations(store, wal.operations.clone());
    }

    let mut truncated_wal = false;
    if options.truncate
        && let Some(wal) = &wal
        && wal
            .corrupt_ranges
            .last()
            .is_some_and(|range| range.1 == wal.size)
    {
        tokio::fs::OpenOptions::new()
            .write(true)
            .open(options.directory.join("db.wal"))
            .await?
            .set_len(wal.corrupt_ranges.last().expect("checked tail").0)
            .await?;
        truncated_wal = true;
    }
    let snapshot_ranges = snapshot
        .as_ref()
        .map(|scan| scan.corrupt_ranges.clone())
        .unwrap_or_default();
    let wal_ranges = wal
        .as_ref()
        .map(|scan| scan.corrupt_ranges.clone())
        .unwrap_or_default();
    let lost_bytes = snapshot_ranges
        .iter()
        .chain(&wal_ranges)
        .map(|(start, end)| end - start)
        .sum();
    Ok(RecoveryInfo {
        snapshot_frames: snapshot.as_ref().map_or(0, |scan| scan.frames),
        wal_frames: wal.as_ref().map_or(0, |scan| scan.frames),
        truncated_wal,
        corrupt_ranges: wal_ranges,
        snapshot_corrupt_ranges: snapshot_ranges,
        lost_bytes,
        wal_scan_end: wal.as_ref().map_or(0, |scan| scan.scan_end),
        wal_device: wal.as_ref().map_or(0, |scan| scan.anchor.device),
        wal_inode: wal.as_ref().map_or(0, |scan| scan.anchor.inode),
    })
}

fn apply_operations(store: &mut Store, operations: Vec<RecoveredOp>) {
    for operation in operations {
        match operation {
            RecoveredOp::Set {
                key,
                value_ref,
                expire_at,
                datetimes,
            } => store.set_ref(key, value_ref, expire_at, datetimes),
            RecoveredOp::Del { key } => {
                store.del(&key);
            }
        }
    }
}

// Original: packages/minidb/src/recovery.ts, catchUpWal().
pub fn catch_up_wal(
    wal_path: impl AsRef<Path>,
    offset: u64,
    anchor: WalAnchor,
    mut apply: impl FnMut(&FrameRef, &mut File) -> Result<(), RecoveryError>,
) -> Result<Option<CatchUpResult>, RecoveryError> {
    let wal_path = wal_path.as_ref();
    let mut file = match OpenOptions::new().read(true).open(wal_path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if metadata_anchor(wal_path, &metadata)? != anchor || offset > metadata.len() {
        return Ok(None);
    }
    let scan = scan_frame_refs(&mut file, CorruptionMode::Strict, offset)?;
    if scan.frames.is_empty() && scan.eof_offset < metadata.len() {
        let len = (metadata.len() - offset).min(MAGIC.len() as u64) as u32;
        if read_at(&mut file, offset, len)? != MAGIC[..len as usize] {
            return Ok(None);
        }
        return Ok(Some(CatchUpResult {
            offset,
            applied_frames: 0,
        }));
    }
    for frame in &scan.frames {
        apply(frame, &mut file)?;
    }
    Ok(Some(CatchUpResult {
        offset: scan.eof_offset,
        applied_frames: scan.frames.len(),
    }))
}

#[cfg(unix)]
fn metadata_anchor(_path: &Path, metadata: &std::fs::Metadata) -> io::Result<WalAnchor> {
    use std::os::unix::fs::MetadataExt;

    Ok(WalAnchor {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(windows)]
fn metadata_anchor(path: &Path, _metadata: &std::fs::Metadata) -> io::Result<WalAnchor> {
    match file_id::get_low_res_file_id(path)? {
        file_id::FileId::LowRes {
            volume_serial_number,
            file_index,
        } => Ok(WalAnchor {
            device: u64::from(volume_serial_number),
            inode: file_index,
        }),
        _ => Err(io::Error::other("unexpected Windows file ID variant")),
    }
}

#[cfg(not(any(unix, windows)))]
fn metadata_anchor(_path: &Path, _metadata: &std::fs::Metadata) -> io::Result<WalAnchor> {
    Ok(WalAnchor {
        device: 0,
        inode: 0,
    })
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    use crate::{
        codec::{Frame, encode_frame},
        store::StoreOptions,
    };

    use super::*;

    #[tokio::test]
    async fn recovers_snapshot_then_wal_and_truncates_torn_tail() {
        let directory = tempfile::tempdir().unwrap();
        let snapshot = encode_frame(&Frame {
            frame_type: TYPE_SET,
            key: b"a".to_vec(),
            value: b"old".to_vec(),
            meta: None,
            expire_at: 0,
        })
        .unwrap();
        tokio::fs::write(directory.path().join("db.snapshot"), snapshot)
            .await
            .unwrap();
        let mut wal = tokio::fs::File::create(directory.path().join("db.wal"))
            .await
            .unwrap();
        wal.write_all(
            &encode_frame(&Frame {
                frame_type: TYPE_SET,
                key: b"a".to_vec(),
                value: b"new".to_vec(),
                meta: None,
                expire_at: 0,
            })
            .unwrap(),
        )
        .await
        .unwrap();
        wal.write_all(b"MD\x01").await.unwrap();
        wal.flush().await.unwrap();
        drop(wal);
        let mut store = Store::new(StoreOptions::default());
        let info = recover(&mut store, RecoveryOptions::new(directory.path()))
            .await
            .unwrap();
        assert_eq!(store.get(b"a").unwrap(), Some(b"new".to_vec()));
        assert!(info.truncated_wal);
        assert_eq!(
            tokio::fs::metadata(directory.path().join("db.wal"))
                .await
                .unwrap()
                .len(),
            info.wal_scan_end
        );
    }
}
