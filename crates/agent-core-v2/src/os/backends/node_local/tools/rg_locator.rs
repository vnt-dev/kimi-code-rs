//! Hybrid system/vendor/cache/download ripgrep binary resolution.
//!
//! Original: `packages/agent-core-v2/src/os/backends/node-local/tools/rgLocator.ts`.

use parking_lot::Mutex;
use std::sync::{Arc, LazyLock};
use std::{
    collections::HashMap,
    error::Error,
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};

use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::{io::AsyncWriteExt, sync::watch};

use crate::_base::utils::{
    abort::{AbortError, AbortSignal},
    hash::sha256_hex,
};

const RG_VERSION: &str = "15.0.0";
const RG_BASE_URL: &str = "https://code.kimi.com/kimi-code/rg";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

static RG_ARCHIVE_SHA256: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        (
            "ripgrep-15.0.0-aarch64-apple-darwin.tar.gz",
            "98bb2e61e7277ba0ea72d2ae2592497fd8d2940934a16b122448d302a6637e3b",
        ),
        (
            "ripgrep-15.0.0-aarch64-pc-windows-msvc.zip",
            "572709c8770cb7f9385d725cb06d2bcd9537ec24d4dd17b1be1d65a876f8b591",
        ),
        (
            "ripgrep-15.0.0-aarch64-unknown-linux-gnu.tar.gz",
            "15f8cc2fab12d88491c54d49f38589922a9d6a7353c29b0a0856727bcdf80754",
        ),
        (
            "ripgrep-15.0.0-x86_64-apple-darwin.tar.gz",
            "44128c733d127ddbda461e01225a68b5f9997cfe7635242a797f645ca674a71a",
        ),
        (
            "ripgrep-15.0.0-x86_64-pc-windows-msvc.zip",
            "21a98bf42c4da97ca543c010e764cc6dec8b9b7538d05f8d21874016385e0860",
        ),
        (
            "ripgrep-15.0.0-x86_64-unknown-linux-musl.tar.gz",
            "253ad0fd5fef0d64cba56c70dccdacc1916d4ed70ad057cc525fcdb0c3bbd2a7",
        ),
    ])
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RgResolutionSource {
    SystemPath,
    Vendor,
    ShareBinCached,
    ShareBinDownloaded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgResolution {
    pub path: PathBuf,
    pub source: RgResolutionSource,
}

#[async_trait]
pub trait RgProbe: Send + Sync {
    async fn exec(&self, args: &[String]) -> i32;
}

#[derive(Clone, Default)]
pub struct EnsureRgPathOptions {
    pub share_dir: Option<PathBuf>,
    pub signal: Option<AbortSignal>,
    pub allow_cached_fallback: bool,
    pub process_path: Option<String>,
}

#[derive(Clone, Debug)]
pub enum RgLocatorError {
    Aborted(Arc<AbortError>),
    Failure(String),
}

impl fmt::Display for RgLocatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Aborted(error) => error.fmt(formatter),
            Self::Failure(message) => formatter.write_str(message),
        }
    }
}

impl Error for RgLocatorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Aborted(error) => Some(error.as_ref()),
            Self::Failure(_) => None,
        }
    }
}

type SharedDownloadResult = Result<RgResolution, String>;
type DownloadReceiver = watch::Receiver<Option<SharedDownloadResult>>;
static DOWNLOAD: LazyLock<Mutex<Option<DownloadReceiver>>> = LazyLock::new(|| Mutex::new(None));

pub async fn ensure_rg_path(
    probe: &dyn RgProbe,
    options: EnsureRgPathOptions,
) -> Result<RgResolution, RgLocatorError> {
    if let Some(signal) = &options.signal {
        signal.throw_if_aborted().map_err(RgLocatorError::Aborted)?;
    }
    let share_dir = options.share_dir.clone().unwrap_or_else(get_share_dir);
    let process_path = options
        .process_path
        .clone()
        .or_else(|| std::env::var("PATH").ok());
    if let Some(existing) = find_existing_rg(
        probe,
        &share_dir,
        options.allow_cached_fallback,
        process_path.as_deref(),
    )
    .await
    {
        return Ok(existing);
    }
    if let Some(signal) = &options.signal {
        signal.throw_if_aborted().map_err(RgLocatorError::Aborted)?;
    }
    if !options.allow_cached_fallback {
        return Err(RgLocatorError::Failure(
            "ripgrep (rg) is not available on PATH".into(),
        ));
    }
    let receiver = download_rg_with_lock(share_dir, process_path).await;
    wait_for_download(receiver, options.signal.as_ref()).await
}

pub async fn find_existing_rg(
    _probe: &dyn RgProbe,
    share_dir: &Path,
    allow_cached_fallback: bool,
    process_path: Option<&str>,
) -> Option<RgResolution> {
    if let Some(system) = find_rg_on_path(process_path).await {
        return Some(RgResolution {
            path: system,
            source: RgResolutionSource::SystemPath,
        });
    }
    if allow_cached_fallback {
        if let Some(vendor) = get_vendor_rg_path(rg_binary_name())
            && is_executable_file(&vendor).await
        {
            return Some(RgResolution {
                path: vendor,
                source: RgResolutionSource::Vendor,
            });
        }
        let cached = share_dir.join("bin").join(rg_binary_name());
        if is_executable_file(&cached).await {
            return Some(RgResolution {
                path: cached,
                source: RgResolutionSource::ShareBinCached,
            });
        }
    }
    None
}

async fn download_rg_with_lock(
    share_dir: PathBuf,
    process_path: Option<String>,
) -> DownloadReceiver {
    let mut active = DOWNLOAD.lock();
    if let Some(receiver) = active.as_ref() {
        return receiver.clone();
    }
    let (sender, receiver) = watch::channel(None);
    *active = Some(receiver.clone());
    drop(active);
    tokio::spawn(async move {
        let result = if let Some(system) = find_rg_on_path(process_path.as_deref()).await {
            Ok(RgResolution {
                path: system,
                source: RgResolutionSource::SystemPath,
            })
        } else {
            let cached = share_dir.join("bin").join(rg_binary_name());
            if is_executable_file(&cached).await {
                Ok(RgResolution {
                    path: cached,
                    source: RgResolutionSource::ShareBinCached,
                })
            } else {
                download_and_install_rg(&share_dir)
                    .await
                    .map(|path| RgResolution {
                        path,
                        source: RgResolutionSource::ShareBinDownloaded,
                    })
                    .map_err(|error| error.to_string())
            }
        };
        sender.send_replace(Some(result));
        *DOWNLOAD.lock() = None;
    });
    receiver
}

async fn wait_for_download(
    mut receiver: DownloadReceiver,
    signal: Option<&AbortSignal>,
) -> Result<RgResolution, RgLocatorError> {
    let wait = async {
        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result;
            }
            if receiver.changed().await.is_err() {
                return Err("ripgrep bootstrap task stopped".into());
            }
        }
    };
    let result = match signal {
        Some(signal) => tokio::select! {
            result = wait => result,
            reason = signal.cancelled() => return Err(RgLocatorError::Aborted(reason)),
        },
        None => wait.await,
    };
    result.map_err(RgLocatorError::Failure)
}

fn rg_binary_name() -> &'static str {
    if cfg!(windows) { "rg.exe" } else { "rg" }
}

fn get_share_dir() -> PathBuf {
    std::env::var_os("KIMI_CODE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".kimi-code")
        })
}

pub fn get_share_bin_rg_path() -> PathBuf {
    get_share_dir().join("bin").join(rg_binary_name())
}

fn get_vendor_rg_path(_: &str) -> Option<PathBuf> {
    None
}

async fn find_rg_on_path(path: Option<&str>) -> Option<PathBuf> {
    for directory in std::env::split_paths(path.unwrap_or_default()) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        let candidate = directory.join(rg_binary_name());
        if is_executable_file(&candidate).await {
            return Some(candidate);
        }
    }
    None
}

async fn is_executable_file(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_file())
}

pub fn detect_target() -> Option<String> {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        value => value,
    };
    detect_target_for(platform, std::env::consts::ARCH)
}

pub fn detect_target_for(platform: &str, arch: &str) -> Option<String> {
    let arch = match arch {
        "x86_64" | "x64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        _ => return None,
    };
    match platform {
        "darwin" => Some(format!("{arch}-apple-darwin")),
        "linux" if arch == "x86_64" => Some("x86_64-unknown-linux-musl".into()),
        "linux" => Some("aarch64-unknown-linux-gnu".into()),
        "win32" => Some(format!("{arch}-pc-windows-msvc")),
        _ => None,
    }
}

async fn download_and_install_rg(share_dir: &Path) -> Result<PathBuf, RgLocatorError> {
    let target = detect_target().ok_or_else(|| {
        RgLocatorError::Failure(format!(
            "Unsupported platform/arch for ripgrep download: {}/{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })?;
    let windows = target.contains("windows");
    let extension = if windows { "zip" } else { "tar.gz" };
    let archive_name = format!("ripgrep-{RG_VERSION}-{target}.{extension}");
    let expected = RG_ARCHIVE_SHA256
        .get(archive_name.as_str())
        .ok_or_else(|| {
            RgLocatorError::Failure(format!(
                "No pinned SHA-256 is configured for ripgrep archive {archive_name}"
            ))
        })?;
    let bin_dir = share_dir.join("bin");
    tokio::fs::create_dir_all(&bin_dir).await.map_err(failure)?;
    let destination = bin_dir.join(rg_binary_name());
    let temporary = std::env::temp_dir().join(format!("kimi-rg-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir(&temporary).await.map_err(failure)?;
    let result = download_extract_install(
        &temporary,
        &bin_dir,
        &destination,
        &archive_name,
        &target,
        expected,
        windows,
    )
    .await;
    let _ = tokio::fs::remove_dir_all(&temporary).await;
    result.map(|_| destination)
}

async fn download_extract_install(
    temporary: &Path,
    bin_dir: &Path,
    destination: &Path,
    archive_name: &str,
    target: &str,
    expected: &str,
    windows: bool,
) -> Result<(), RgLocatorError> {
    let archive_path = temporary.join(archive_name);
    let url = format!("{RG_BASE_URL}/{archive_name}");
    let response = tokio::time::timeout(DOWNLOAD_TIMEOUT, reqwest::get(&url))
        .await
        .map_err(|_| RgLocatorError::Failure("ripgrep download timed out".into()))?
        .map_err(failure)?;
    if !response.status().is_success() {
        return Err(RgLocatorError::Failure(
            format!(
                "Failed to download ripgrep: HTTP {} {}",
                response.status().as_u16(),
                response.status().canonical_reason().unwrap_or_default()
            )
            .trim_end()
            .into(),
        ));
    }
    let mut file = tokio::fs::File::create(&archive_path)
        .await
        .map_err(failure)?;
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        file.write_all(&chunk.map_err(failure)?)
            .await
            .map_err(failure)?;
    }
    file.flush().await.map_err(failure)?;
    drop(file);
    verify_archive_checksum(&archive_path, archive_name, expected).await?;
    if windows {
        extract_rg_from_zip(&archive_path, destination).await
    } else {
        let extracted = temporary
            .join("extract")
            .join(format!("ripgrep-{RG_VERSION}-{target}"))
            .join(rg_binary_name());
        extract_rg_from_tar(&archive_path, &extracted).await?;
        if !tokio::fs::try_exists(&extracted).await.map_err(failure)? {
            return Err(RgLocatorError::Failure(format!(
                "Ripgrep archive did not contain expected binary at {}. CDN content may have changed.",
                extracted.display()
            )));
        }
        let install_dir = bin_dir.join(format!(".rg-install-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&install_dir).await.map_err(failure)?;
        let staged = install_dir.join(rg_binary_name());
        let install_result = async {
            tokio::fs::copy(&extracted, &staged)
                .await
                .map_err(failure)?;
            set_executable(&staged).await?;
            tokio::fs::rename(&staged, destination)
                .await
                .map_err(failure)
        }
        .await;
        let _ = tokio::fs::remove_dir_all(install_dir).await;
        install_result
    }
}

pub async fn verify_archive_checksum(
    archive_path: &Path,
    archive_name: &str,
    expected: &str,
) -> Result<(), RgLocatorError> {
    let bytes = tokio::fs::read(archive_path).await.map_err(failure)?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(RgLocatorError::Failure(format!(
            "Ripgrep archive checksum mismatch for {archive_name}: expected {expected}, got {actual}. CDN content may have changed."
        )));
    }
    Ok(())
}

pub async fn extract_rg_from_zip(
    archive_path: &Path,
    destination: &Path,
) -> Result<(), RgLocatorError> {
    let archive_path = archive_path.to_path_buf();
    let destination = destination.to_path_buf();
    tokio::task::spawn_blocking(move || extract_zip_blocking(&archive_path, &destination))
        .await
        .map_err(failure)?
}

fn extract_zip_blocking(archive_path: &Path, destination: &Path) -> Result<(), RgLocatorError> {
    let file = std::fs::File::open(archive_path).map_err(failure)?;
    let mut archive = zip::ZipArchive::new(file).map_err(failure)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(failure)?;
        if Path::new(entry.name())
            .file_name()
            .and_then(|name| name.to_str())
            != Some(rg_binary_name())
        {
            continue;
        }
        let mut output = std::fs::File::create(destination).map_err(failure)?;
        std::io::copy(&mut entry, &mut output).map_err(failure)?;
        return Ok(());
    }
    Err(RgLocatorError::Failure(format!(
        "Ripgrep archive did not contain expected binary '{}'. CDN content may have changed.",
        rg_binary_name()
    )))
}

async fn extract_rg_from_tar(
    archive_path: &Path,
    destination: &Path,
) -> Result<(), RgLocatorError> {
    let archive_path = archive_path.to_path_buf();
    let destination = destination.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(archive_path).map_err(failure)?;
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        for entry in archive.entries().map_err(failure)? {
            let mut entry = entry.map_err(failure)?;
            if entry
                .path()
                .map_err(failure)?
                .file_name()
                .and_then(|name| name.to_str())
                != Some(rg_binary_name())
            {
                continue;
            }
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(failure)?;
            }
            let mut output = std::fs::File::create(&destination).map_err(failure)?;
            std::io::copy(&mut entry, &mut output).map_err(failure)?;
            return Ok(());
        }
        Ok(())
    })
    .await
    .map_err(failure)?
}

#[cfg(unix)]
async fn set_executable(path: &Path) -> Result<(), RgLocatorError> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .await
        .map_err(failure)
}

#[cfg(not(unix))]
async fn set_executable(_: &Path) -> Result<(), RgLocatorError> {
    Ok(())
}

fn failure(error: impl fmt::Display) -> RgLocatorError {
    RgLocatorError::Failure(error.to_string())
}

pub fn rg_unavailable_message(cause: impl fmt::Display) -> String {
    format!(
        "ripgrep (rg) is not available and the automatic bootstrap failed.\n\nError: {cause}\n\nFix options:\n  macOS:   brew install ripgrep\n  Ubuntu:  sudo apt-get install ripgrep\n  Other:   https://github.com/BurntSushi/ripgrep#installation\n\nAlternatively, drop a static rg binary at {}",
        get_share_bin_rg_path().display()
    )
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    struct NoProbe;
    #[async_trait]
    impl RgProbe for NoProbe {
        async fn exec(&self, _: &[String]) -> i32 {
            -1
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kimi-rg-{label}-{}-{nonce}", std::process::id()))
    }

    #[tokio::test]
    async fn lookup_prefers_path_then_cache_and_never_executes_probe() {
        let root = temp_dir("lookup");
        let system_dir = root.join("system");
        let cache_dir = root.join("share/bin");
        tokio::fs::create_dir_all(&system_dir).await.unwrap();
        tokio::fs::create_dir_all(&cache_dir).await.unwrap();
        let system = system_dir.join(rg_binary_name());
        let cached = cache_dir.join(rg_binary_name());
        tokio::fs::write(&system, "system").await.unwrap();
        tokio::fs::write(&cached, "cached").await.unwrap();
        let path = std::env::join_paths([&system_dir])
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let found = find_existing_rg(&NoProbe, &root.join("share"), true, Some(&path))
            .await
            .unwrap();
        assert_eq!(
            found,
            RgResolution {
                path: system,
                source: RgResolutionSource::SystemPath
            }
        );
        let found = find_existing_rg(&NoProbe, &root.join("share"), true, Some(""))
            .await
            .unwrap();
        assert_eq!(
            found,
            RgResolution {
                path: cached,
                source: RgResolutionSource::ShareBinCached
            }
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[test]
    fn target_matrix_and_unavailable_message_match_source() {
        assert_eq!(
            detect_target_for("darwin", "arm64").as_deref(),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(
            detect_target_for("linux", "x64").as_deref(),
            Some("x86_64-unknown-linux-musl")
        );
        assert_eq!(
            detect_target_for("win32", "x64").as_deref(),
            Some("x86_64-pc-windows-msvc")
        );
        assert_eq!(detect_target_for("linux", "mips"), None);
        let message = rg_unavailable_message("fetch failed");
        assert!(message.contains("automatic bootstrap failed"));
        assert!(message.contains("brew install ripgrep"));
    }

    #[tokio::test]
    async fn checksum_accepts_trusted_and_rejects_tampered_bytes() {
        let root = temp_dir("sha");
        tokio::fs::create_dir(&root).await.unwrap();
        let archive = root.join("archive.tar.gz");
        tokio::fs::write(&archive, "trusted archive bytes")
            .await
            .unwrap();
        let expected = sha256_hex(b"trusted archive bytes");
        verify_archive_checksum(&archive, "archive.tar.gz", &expected)
            .await
            .unwrap();
        assert!(
            verify_archive_checksum(&archive, "archive.tar.gz", &"0".repeat(64))
                .await
                .unwrap_err()
                .to_string()
                .contains("checksum mismatch")
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn zip_extracts_only_named_binary_and_rejects_missing_entry() {
        let root = temp_dir("zip");
        tokio::fs::create_dir(&root).await.unwrap();
        let archive = root.join("fixture.zip");
        let archive_for_write = archive.clone();
        tokio::task::spawn_blocking(move || {
            let file = std::fs::File::create(archive_for_write).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file(
                format!("folder/{}", rg_binary_name()),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"binary").unwrap();
            zip.finish().unwrap();
        })
        .await
        .unwrap();
        let destination = root.join(rg_binary_name());
        extract_rg_from_zip(&archive, &destination).await.unwrap();
        assert_eq!(tokio::fs::read(destination).await.unwrap(), b"binary");

        let missing = root.join("missing.zip");
        let missing_for_write = missing.clone();
        tokio::task::spawn_blocking(move || {
            let file = std::fs::File::create(missing_for_write).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("README.md", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"readme").unwrap();
            zip.finish().unwrap();
        })
        .await
        .unwrap();
        assert!(
            extract_rg_from_zip(&missing, &root.join("missing-rg"))
                .await
                .unwrap_err()
                .to_string()
                .contains("CDN content may have changed")
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn ensure_without_fallback_or_with_preexisting_abort_never_bootstraps() {
        let root = temp_dir("ensure");
        tokio::fs::create_dir(&root).await.unwrap();
        let error = ensure_rg_path(
            &NoProbe,
            EnsureRgPathOptions {
                share_dir: Some(root.clone()),
                process_path: Some(String::new()),
                ..EnsureRgPathOptions::default()
            },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("on PATH"));

        let controller = crate::_base::utils::abort::AbortController::new();
        controller.abort(None);
        let error = ensure_rg_path(
            &NoProbe,
            EnsureRgPathOptions {
                share_dir: Some(root.clone()),
                signal: Some(controller.signal()),
                allow_cached_fallback: true,
                process_path: Some(String::new()),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, RgLocatorError::Aborted(_)));
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
