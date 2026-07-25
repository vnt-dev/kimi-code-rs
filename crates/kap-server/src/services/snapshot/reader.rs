use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use kimi_code_protocol::rest::snapshot::SessionSnapshotResponse;
use thiserror::Error;
use tokio::fs;

use super::config::SnapshotConfig;

const BLOBREF_PROTOCOL: &str = "blobref:";

#[derive(Debug, Error)]
#[error("session {session_id} does not exist")]
pub struct SnapshotNotFoundError {
    pub session_id: String,
}

#[derive(Debug, Error)]
#[error("snapshot {session_id} timed out after {timeout_ms}ms")]
pub struct SnapshotTimeoutError {
    pub session_id: String,
    pub timeout_ms: u64,
}

#[derive(Debug)]
pub struct SnapshotReader {
    pub home_dir: PathBuf,
    pub config: SnapshotConfig,
}

impl SnapshotReader {
    pub fn new(home_dir: impl AsRef<Path>, config: SnapshotConfig) -> Self {
        Self {
            home_dir: home_dir.as_ref().to_owned(),
            config,
        }
    }

    pub async fn read(&self, _session_id: &str) -> SessionSnapshotResponse {
        // MIGRATION-TODO:
        // Original: services/snapshot/snapshotReader.ts, SnapshotReader.read()
        // Missing dependency: agent-core-v2 ISessionIndex,
        // IWorkspaceRegistry, ISessionLifecycleService, context transcript
        // reducer, and session interaction service.
        // Implemented independently below: config parsing, JSONL parsing and
        // blobref rehydration. Complete this orchestration when those core-v2
        // contracts are available.
        todo!("assemble snapshots after kimi-code-agent-core-v2 is complete")
    }
}

#[derive(Debug, Error)]
pub enum WireReadError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("wire.jsonl: corrupted line {line} in {}: {source}", path.display())]
    Corrupt {
        path: PathBuf,
        line: usize,
        source: serde_json::Error,
    },
}

// Original: snapshotReader.ts, readWireRecords().
pub async fn read_wire_records(
    wire_path: impl AsRef<Path>,
) -> Result<Vec<serde_json::Value>, WireReadError> {
    let wire_path = wire_path.as_ref();
    let raw = fs::read_to_string(wire_path).await?;
    let lines: Vec<&str> = raw.split('\n').collect();
    let mut records = Vec::new();
    for (index, raw_line) in lines.iter().enumerate() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str(line) {
            Ok(record) => records.push(record),
            Err(_) if index == lines.len() - 1 => break,
            Err(source) => {
                return Err(WireReadError::Corrupt {
                    path: wire_path.to_owned(),
                    line: index + 1,
                    source,
                });
            }
        }
    }
    Ok(records)
}

/// Resolve and cache a `blobref:<mime>;<sha256>` media reference.
pub async fn resolve_blob_ref(
    url: &str,
    blobs_dir: impl AsRef<Path>,
    cache: &mut HashMap<String, Option<String>>,
) -> Option<String> {
    if let Some(resolved) = cache.get(url) {
        return resolved.clone();
    }
    let resolved = resolve_blob_ref_uncached(url, blobs_dir.as_ref()).await;
    cache.insert(url.to_owned(), resolved.clone());
    resolved
}

async fn resolve_blob_ref_uncached(url: &str, blobs_dir: &Path) -> Option<String> {
    let rest = url.strip_prefix(BLOBREF_PROTOCOL)?;
    let (mime_type, hash) = rest.split_once(';')?;
    if hash.len() < 16 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let payload = fs::read(blobs_dir.join(hash)).await.ok()?;
    Some(format!(
        "data:{mime_type};base64,{}",
        STANDARD.encode(payload)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drops_torn_final_line_and_rejects_mid_file_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wire.jsonl");
        fs::write(
            &path,
            "{\"type\":\"context.append_message\"}\n{\"type\":\"context.append",
        )
        .await
        .unwrap();
        let records = read_wire_records(&path).await.unwrap();
        assert_eq!(records.len(), 1);

        fs::write(&path, "{not-json}\n{\"type\":\"valid\"}\n")
            .await
            .unwrap();
        let error = read_wire_records(&path).await.unwrap_err();
        assert!(error.to_string().contains("corrupted line 1"));
    }

    #[tokio::test]
    async fn resolves_valid_blob_refs_and_caches_missing_values() {
        let directory = tempfile::tempdir().unwrap();
        let hash = "0123456789abcdef";
        fs::write(directory.path().join(hash), b"hello")
            .await
            .unwrap();
        let mut cache = HashMap::new();
        assert_eq!(
            resolve_blob_ref(
                &format!("blobref:text/plain;{hash}"),
                directory.path(),
                &mut cache
            )
            .await,
            Some("data:text/plain;base64,aGVsbG8=".into())
        );
        assert_eq!(
            resolve_blob_ref("blobref:text/plain;bad", directory.path(), &mut cache).await,
            None
        );
        assert!(cache.contains_key("blobref:text/plain;bad"));
    }
}
