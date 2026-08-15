//! Plugin ZIP download and extraction.
//!
//! Original: `packages/agent-core-v2/src/app/plugin/archive.ts`.

use std::{
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use futures_util::StreamExt;
use thiserror::Error;

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Error)]
pub enum PluginArchiveError {
    #[error("Failed to download zip: HTTP {status} {status_text}")]
    HttpStatus { status: u16, status_text: String },
    #[error("plugin zip download timed out")]
    Timeout,
    #[error("failed to download plugin zip: {0}")]
    Download(#[from] reqwest::Error),
    #[error("Failed to open zip: {0}")]
    OpenZip(zip::result::ZipError),
    #[error("Path traversal detected in zip entry: {0}")]
    PathTraversal(String),
    #[error("Failed to read {entry} from archive: {source}")]
    ReadEntry {
        entry: String,
        #[source]
        source: zip::result::ZipError,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("plugin ZIP extraction task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

// Original: archive.ts, downloadZip(). Cancellation of this future cancels the
// request; the source's internal controller contributes a five-minute timeout.
pub async fn download_zip(url: &str) -> Result<Vec<u8>, PluginArchiveError> {
    download_zip_with_progress(url, None).await
}

pub async fn download_zip_with_progress(
    url: &str,
    progress: Option<&(dyn Fn(u64, Option<u64>) + Send + Sync)>,
) -> Result<Vec<u8>, PluginArchiveError> {
    tokio::time::timeout(DOWNLOAD_TIMEOUT, download_zip_inner(url, progress))
        .await
        .map_err(|_| PluginArchiveError::Timeout)?
}

async fn download_zip_inner(
    url: &str,
    progress: Option<&(dyn Fn(u64, Option<u64>) + Send + Sync)>,
) -> Result<Vec<u8>, PluginArchiveError> {
    let response = reqwest::get(url).await?;
    let status = response.status();
    if !status.is_success() {
        return Err(PluginArchiveError::HttpStatus {
            status: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or_default().to_owned(),
        });
    }
    let total_bytes = response.content_length();
    if let Some(progress) = progress {
        progress(0, total_bytes);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    let mut downloaded_bytes = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        downloaded_bytes = downloaded_bytes.saturating_add(chunk.len() as u64);
        bytes.extend_from_slice(&chunk);
        if let Some(progress) = progress {
            progress(downloaded_bytes, total_bytes);
        }
    }
    Ok(bytes)
}

// Original: archive.ts, extractZip(). The synchronous `zip` reader and file
// writes are isolated from async runtime workers as one blocking operation.
pub async fn extract_zip(
    buffer: Vec<u8>,
    destination: impl AsRef<Path>,
) -> Result<String, PluginArchiveError> {
    let destination = destination.as_ref().to_owned();
    let extraction_destination = destination.clone();
    tokio::task::spawn_blocking(move || extract_zip_blocking(&buffer, &extraction_destination))
        .await??;
    detect_plugin_root(&destination).await
}

fn extract_zip_blocking(buffer: &[u8], destination: &Path) -> Result<(), PluginArchiveError> {
    std::fs::create_dir_all(destination)?;
    let destination = std::path::absolute(destination)?;
    let mut archive =
        zip::ZipArchive::new(Cursor::new(buffer)).map_err(PluginArchiveError::OpenZip)?;

    for index in 0..archive.len() {
        let mut entry =
            archive
                .by_index(index)
                .map_err(|source| PluginArchiveError::ReadEntry {
                    entry: format!("entry {index}"),
                    source,
                })?;
        let name = entry.name().to_owned();
        let destination_path = resolve_archive_path(&destination, &name)
            .ok_or_else(|| PluginArchiveError::PathTraversal(name.clone()))?;

        if entry.is_dir() || name.ends_with('/') {
            std::fs::create_dir_all(&destination_path)?;
            continue;
        }
        if let Some(parent) = destination_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&destination_path)?;
        let mut chunk = [0_u8; 16 * 1024];
        loop {
            let count = entry.read(&mut chunk).map_err(PluginArchiveError::Io)?;
            if count == 0 {
                break;
            }
            file.write_all(&chunk[..count])?;
        }
        // Unix permission bits are a POSIX concept; the Windows extract
        // needs no chmod, so the step is compiled out there.
        #[cfg(unix)]
        restore_file_permissions(&destination_path, entry.unix_mode())?;
    }
    Ok(())
}

fn resolve_archive_path(destination: &Path, name: &str) -> Option<PathBuf> {
    let entry = Path::new(name);
    if entry.is_absolute()
        || entry
            .components()
            .any(|component| matches!(component, Component::Prefix(_)))
    {
        return None;
    }
    let mut resolved = destination.to_owned();
    for component in entry.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if resolved == destination {
                    return None;
                }
                resolved.pop();
            }
            Component::Normal(value) => resolved.push(value),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (resolved == destination || resolved.starts_with(destination)).then_some(resolved)
}

#[cfg(unix)]
fn restore_file_permissions(path: &Path, mode: Option<u32>) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;

    let Some(permissions) = mode.map(|mode| mode & 0o777).filter(|mode| *mode != 0) else {
        return Ok(());
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(permissions))
}

// Original: archive.ts, detectPluginRoot().
async fn detect_plugin_root(directory: &Path) -> Result<String, PluginArchiveError> {
    if has_manifest(directory).await {
        return Ok(path_to_string(directory));
    }
    let mut entries = tokio::fs::read_dir(directory).await?;
    let mut child_directories = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_dir() {
            child_directories.push(entry.path());
        }
    }
    if child_directories.len() == 1 && has_manifest(&child_directories[0]).await {
        return Ok(path_to_string(&child_directories[0]));
    }
    Ok(path_to_string(directory))
}

async fn has_manifest(directory: &Path) -> bool {
    is_file(&directory.join("kimi.plugin.json")).await
        || is_file(&directory.join(".kimi-plugin/plugin.json")).await
}

async fn is_file(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_file())
}

fn path_to_string(path: impl AsRef<Path>) -> String {
    path.as_ref().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use zip::write::SimpleFileOptions;

    use super::*;

    fn archive(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content, mode) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default().unix_permissions(*mode))
                .unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[tokio::test]
    async fn reports_downloaded_bytes_and_content_length() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let body = b"plugin archive bytes".to_vec();
        let response_body = body.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            );
            socket.write_all(headers.as_bytes()).await.unwrap();
            socket.write_all(&response_body).await.unwrap();
        });

        let events = Mutex::new(Vec::new());
        let progress = |downloaded_bytes, total_bytes| {
            events.lock().push((downloaded_bytes, total_bytes));
        };
        let downloaded =
            download_zip_with_progress(&format!("http://{address}/plugin.zip"), Some(&progress))
                .await
                .unwrap();
        server.await.unwrap();

        assert_eq!(downloaded, body);
        let events = events.into_inner();
        assert_eq!(events.first(), Some(&(0, Some(body.len() as u64))));
        assert_eq!(
            events.last(),
            Some(&(body.len() as u64, Some(body.len() as u64)))
        );
    }

    #[tokio::test]
    async fn extracts_files_and_detects_a_single_wrapping_plugin_directory() {
        let destination = std::env::temp_dir().join(format!("plugin-zip-{}", uuid::Uuid::new_v4()));
        let bytes = archive(&[
            ("wrapper/kimi.plugin.json", br#"{"name":"demo"}"#, 0o644),
            ("wrapper/bin/run", b"run", 0o755),
        ]);
        let root = extract_zip(bytes, &destination).await.unwrap();
        assert_eq!(Path::new(&root), destination.join("wrapper"));
        assert_eq!(
            tokio::fs::read(destination.join("wrapper/bin/run"))
                .await
                .unwrap(),
            b"run"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(destination.join("wrapper/bin/run"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
        }
        tokio::fs::remove_dir_all(destination).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_path_traversal_before_writing_the_entry() {
        let destination = std::env::temp_dir().join(format!("plugin-zip-{}", uuid::Uuid::new_v4()));
        let error = extract_zip(archive(&[("../escape.txt", b"bad", 0o644)]), &destination)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Path traversal detected in zip entry: ../escape.txt"
        );
        assert!(!destination.parent().unwrap().join("escape.txt").exists());
        tokio::fs::remove_dir_all(destination).await.unwrap();
    }
}
