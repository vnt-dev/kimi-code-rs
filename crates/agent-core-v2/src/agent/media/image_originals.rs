use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
    time::SystemTime,
};

use sha2::{Digest, Sha256};
use tokio::fs;

// Original:
//   packages/agent-core-v2/src/agent/media/image-originals.ts
//   originalImageCacheDir(), sessionMediaOriginalsDir(), persistOriginalImage(), sweepCache()
//
// Rust adaptation:
//   Filesystem work remains asynchronous. PathBuf is used at the Rust boundary;
//   persist_original_image returns its display path because that value is embedded
//   in the model-visible compression caption.
pub const DEFAULT_MAX_TOTAL_BYTES: f64 = 1024.0 * 1024.0 * 1024.0;

#[derive(Clone, Debug, Default)]
pub struct PersistOriginalImageOptions {
    pub dir: Option<PathBuf>,
    pub max_total_bytes: Option<f64>,
}

pub fn original_image_cache_dir() -> PathBuf {
    std::env::temp_dir().join("kimi-code-original-images")
}

pub fn session_media_originals_dir(session_dir: impl AsRef<Path>) -> PathBuf {
    session_dir.as_ref().join("media-originals")
}

// Original: persistOriginalImage(). All filesystem failures deliberately
// collapse to None: preserving an original must never block prompt delivery.
pub async fn persist_original_image(
    bytes: &[u8],
    mime_type: &str,
    options: &PersistOriginalImageOptions,
) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let dir = options.dir.clone().unwrap_or_else(original_image_cache_dir);
    let max_total_bytes = options.max_total_bytes.unwrap_or(DEFAULT_MAX_TOTAL_BYTES);
    let hash = hex_sha256(bytes);
    let extension = mime_extension(mime_type).unwrap_or("img");
    let path = dir.join(format!("{hash}.{extension}"));

    if fs::create_dir_all(&dir).await.is_err() {
        return None;
    }
    let existing = fs::metadata(&path).await.ok();
    if existing.is_none_or(|metadata| metadata.len() != bytes.len() as u64)
        && fs::write(&path, bytes).await.is_err()
    {
        return None;
    }

    if sweep_cache(&dir, max_total_bytes).await.is_err() {
        return None;
    }
    fs::metadata(&path)
        .await
        .ok()
        .map(|_| path.to_string_lossy().into_owned())
}

struct CacheEntry {
    path: PathBuf,
    size: f64,
    modified: SystemTime,
}

// Original: sweepCache(). The stable sort preserves directory enumeration
// order for equal mtimes, matching JavaScript's stable Array#sort behavior.
async fn sweep_cache(dir: &Path, max_total_bytes: f64) -> std::io::Result<()> {
    let mut names = fs::read_dir(dir).await?;
    let mut entries = Vec::new();
    while let Some(entry) = names.next_entry().await? {
        let path = entry.path();
        let Ok(metadata) = fs::metadata(&path).await else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        entries.push(CacheEntry {
            path,
            size: metadata.len() as f64,
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        });
    }
    let mut total = entries.iter().map(|entry| entry.size).sum::<f64>();
    if total <= max_total_bytes {
        return Ok(());
    }
    entries.sort_by(|left, right| match left.modified.cmp(&right.modified) {
        Ordering::Equal => Ordering::Equal,
        order => order,
    });
    for entry in entries {
        if total <= max_total_bytes {
            break;
        }
        let _ = fs::remove_file(&entry.path).await;
        total -= entry.size;
    }
    Ok(())
}

fn mime_extension(mime_type: &str) -> Option<&'static str> {
    match mime_type.trim().to_ascii_lowercase().as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/bmp" => Some("bmp"),
        "image/tiff" => Some("tif"),
        _ => None,
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::*;

    fn test_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kimi-code-image-originals-test-{unique}"))
    }

    #[test]
    fn path_helpers_match_the_source_layout() {
        assert_eq!(
            original_image_cache_dir(),
            std::env::temp_dir().join("kimi-code-original-images")
        );
        assert_eq!(
            session_media_originals_dir("/sessions/one"),
            PathBuf::from("/sessions/one/media-originals")
        );
    }

    #[tokio::test]
    async fn persists_content_addressed_files_with_a_mime_extension() {
        let dir = test_dir();
        let options = PersistOriginalImageOptions {
            dir: Some(dir.clone()),
            ..Default::default()
        };
        let first = persist_original_image(b"image", " IMAGE/JPEG ", &options)
            .await
            .unwrap();
        let duplicate = persist_original_image(b"image", "image/jpeg", &options)
            .await
            .unwrap();
        assert_eq!(first, duplicate);
        assert!(first.ends_with(".jpg"));
        assert_eq!(fs::read(&first).await.unwrap(), b"image");
        assert_eq!(
            persist_original_image(b"", "image/png", &options).await,
            None
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn sweeps_oldest_files_after_a_write_and_returns_none_if_the_new_file_is_removed() {
        let dir = test_dir();
        let roomy = PersistOriginalImageOptions {
            dir: Some(dir.clone()),
            max_total_bytes: Some(100.0),
        };
        let old = persist_original_image(b"older", "image/png", &roomy)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let constrained = PersistOriginalImageOptions {
            dir: Some(dir.clone()),
            max_total_bytes: Some(7.0),
        };
        let new = persist_original_image(b"newest", "image/png", &constrained)
            .await
            .unwrap();
        assert!(!Path::new(&old).exists());
        assert!(Path::new(&new).exists());

        let removed = persist_original_image(
            b"too-large",
            "image/png",
            &PersistOriginalImageOptions {
                dir: Some(dir.clone()),
                max_total_bytes: Some(1.0),
            },
        )
        .await;
        assert_eq!(removed, None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
