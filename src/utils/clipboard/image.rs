use std::{
    collections::HashMap,
    error::Error,
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};

use async_trait::async_trait;
use url::Url;
use uuid::Uuid;

use crate::utils::image::parse_image_meta;

use super::common::{
    CommandOutput, DEFAULT_LIST_TIMEOUT, SUPPORTED_IMAGE_MIME_TYPES, base_mime_type, detect_wsl,
    is_supported_image_mime_type, is_wayland_session, parse_target_list, run_command_async,
};

const MAX_VIDEO_BYTES: u64 = 100 * 1_024 * 1_024;
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_POWERSHELL_TIMEOUT: Duration = Duration::from_secs(5);

const MACOS_FILE_PATH_SCRIPT: &str = r#"ObjC.import('AppKit');
ObjC.import('Foundation');
const out = [];
const pb = $.NSPasteboard.generalPasteboard;
if (String(pb) !== '[id nil]') {
  try {
    const options = $.NSMutableDictionary.dictionary;
    options.setObjectForKey($.NSNumber.numberWithBool(true), $.NSPasteboardURLReadingFileURLsOnlyKey);
    const urls = pb.readObjectsForClassesOptions([$.NSURL], options);
    const count = urls ? urls.count : 0;
    for (let i = 0; i < count; i++) {
      const value = urls.objectAtIndex(i).path;
      const path = value ? ObjC.unwrap(value) : '';
      if (path) out.push(path);
    }
  } catch (error) {}
  if (out.length === 0) {
    try {
      const files = ObjC.deepUnwrap(pb.propertyListForType('NSFilenamesPboardType'));
      if (Array.isArray(files)) for (const path of files) if (path) out.push(String(path));
      else if (files) out.push(String(files));
    } catch (error) {}
  }
}
out.join('\n');"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardVideo {
    pub mime_type: String,
    pub filename: String,
    pub source_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardMedia {
    Image(ClipboardImage),
    Video(ClipboardVideo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardMediaError {
    size: u64,
}

impl fmt::Display for ClipboardMediaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Video is {:.1} MB; maximum supported size is 100 MB.",
            self.size as f64 / 1_024.0 / 1_024.0
        )
    }
}

impl Error for ClipboardMediaError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardPlatform {
    Linux,
    MacOs,
    Windows,
    Other,
}

impl ClipboardPlatform {
    pub fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

#[async_trait]
pub trait ClipboardCommandRunner: Send + Sync {
    async fn run(
        &self,
        command: &str,
        args: &[String],
        timeout: Duration,
        environment: Option<&HashMap<String, String>>,
    ) -> CommandOutput;
}

pub struct SystemClipboardCommandRunner;

#[async_trait]
impl ClipboardCommandRunner for SystemClipboardCommandRunner {
    async fn run(
        &self,
        command: &str,
        args: &[String],
        timeout: Duration,
        environment: Option<&HashMap<String, String>>,
    ) -> CommandOutput {
        run_command_async(command, args, Some(timeout), environment).await
    }
}

pub async fn read_clipboard_media() -> Result<Option<ClipboardMedia>, ClipboardMediaError> {
    let environment = std::env::vars().collect::<HashMap<_, _>>();
    read_clipboard_media_with(
        &SystemClipboardCommandRunner,
        ClipboardPlatform::current(),
        &environment,
        detect_wsl(&environment),
    )
    .await
}

/// Reads a supported image or video using the source platform fallback order.
///
/// Original:
///   apps/kimi-code/src/utils/clipboard/clipboard-image.ts
///   readClipboardMedia()
///
/// Rust adaptation:
///   Windows image access uses the same PowerShell PNG bridge as WSL because
///   Rust has no optional Node native binding. Linux retains wl-paste then
///   xclip behavior; macOS file URLs retain the AppKit osascript path.
pub async fn read_clipboard_media_with(
    runner: &dyn ClipboardCommandRunner,
    platform: ClipboardPlatform,
    environment: &HashMap<String, String>,
    wsl: bool,
) -> Result<Option<ClipboardMedia>, ClipboardMediaError> {
    if environment.contains_key("TERMUX_VERSION") {
        return Ok(None);
    }
    match platform {
        ClipboardPlatform::Linux => {
            let wayland = is_wayland_session(environment);
            if wayland || wsl {
                if let Some(media) = read_file_media_via_wl_paste(runner).await? {
                    return Ok(Some(media));
                }
                if let Some(media) = read_file_media_via_xclip(runner).await? {
                    return Ok(Some(media));
                }
                if let Some(image) = read_image_via_wl_paste(runner).await {
                    return accepted_image(image);
                }
                if let Some(image) = read_image_via_xclip(runner).await {
                    return accepted_image(image);
                }
            }
            if wsl && let Some(image) = read_image_via_powershell(runner, environment, true).await {
                return accepted_image(image);
            }
            if !wayland {
                if let Some(media) = read_file_media_via_xclip(runner).await? {
                    return Ok(Some(media));
                }
                if let Some(image) = read_image_via_xclip(runner).await {
                    return accepted_image(image);
                }
            }
            Ok(None)
        }
        ClipboardPlatform::MacOs => {
            let paths = read_macos_file_paths(runner).await;
            Ok(read_media_from_paths(&paths).await?)
        }
        ClipboardPlatform::Windows => {
            match read_image_via_powershell(runner, environment, false).await {
                Some(image) => accepted_image(image),
                None => Ok(None),
            }
        }
        ClipboardPlatform::Other => Ok(None),
    }
}

fn accepted_image(image: ClipboardImage) -> Result<Option<ClipboardMedia>, ClipboardMediaError> {
    Ok(is_supported_image_mime_type(&image.mime_type).then_some(ClipboardMedia::Image(image)))
}

pub fn select_preferred_image_mime_type(candidates: &[String]) -> Option<String> {
    let normalized = candidates
        .iter()
        .map(|raw| (raw, base_mime_type(raw)))
        .filter(|(raw, _)| !raw.trim().is_empty())
        .collect::<Vec<_>>();
    for preferred in SUPPORTED_IMAGE_MIME_TYPES {
        if let Some((raw, _)) = normalized.iter().find(|(_, base)| base == preferred) {
            return Some(raw.trim().to_owned());
        }
    }
    normalized
        .into_iter()
        .find(|(_, base)| base.starts_with("image/"))
        .map(|(raw, _)| raw.trim().to_owned())
}

pub fn parse_clipboard_paths(text: &str) -> Vec<PathBuf> {
    split_clipboard_path_lines(text)
        .into_iter()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            if line.starts_with("file://") {
                Url::parse(line).ok()?.to_file_path().ok()
            } else {
                let path = PathBuf::from(line);
                path.is_absolute().then_some(path)
            }
        })
        .collect()
}

fn split_clipboard_path_lines(text: &str) -> Vec<&str> {
    text.split(['\r', '\n', '\0']).collect()
}

async fn read_media_path(path: &Path) -> Result<Option<ClipboardMedia>, ClipboardMediaError> {
    if let Some(mime_type) = video_mime_from_path(path) {
        let Ok(metadata) = tokio::fs::metadata(path).await else {
            return Ok(None);
        };
        if !metadata.is_file() {
            return Ok(None);
        }
        if metadata.len() > MAX_VIDEO_BYTES {
            return Err(ClipboardMediaError {
                size: metadata.len(),
            });
        }
        return Ok(Some(ClipboardMedia::Video(ClipboardVideo {
            mime_type: mime_type.to_owned(),
            filename: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            source_path: path.to_path_buf(),
        })));
    }
    let Ok(bytes) = tokio::fs::read(path).await else {
        return Ok(None);
    };
    let Some(meta) = parse_image_meta(&bytes) else {
        return Ok(None);
    };
    Ok(Some(ClipboardMedia::Image(ClipboardImage {
        bytes,
        mime_type: meta.mime.as_str().to_owned(),
    })))
}

async fn read_media_from_paths(
    paths: &[PathBuf],
) -> Result<Option<ClipboardMedia>, ClipboardMediaError> {
    for path in paths {
        if let Some(media) = read_media_path(path).await? {
            return Ok(Some(media));
        }
    }
    Ok(None)
}

async fn read_media_from_text(text: &str) -> Result<Option<ClipboardMedia>, ClipboardMediaError> {
    read_media_from_paths(&parse_clipboard_paths(text)).await
}

async fn read_file_media_via_wl_paste(
    runner: &dyn ClipboardCommandRunner,
) -> Result<Option<ClipboardMedia>, ClipboardMediaError> {
    let list = runner
        .run(
            "wl-paste",
            &["--list-types".to_owned()],
            DEFAULT_LIST_TIMEOUT,
            None,
        )
        .await;
    let Some(uri_type) = list
        .ok
        .then(|| parse_target_list(&list.stdout))
        .into_iter()
        .flatten()
        .find(|target| base_mime_type(target) == "text/uri-list")
    else {
        return Ok(None);
    };
    let output = runner
        .run(
            "wl-paste",
            &["--type".to_owned(), uri_type, "--no-newline".to_owned()],
            DEFAULT_READ_TIMEOUT,
            None,
        )
        .await;
    if output.ok {
        read_media_from_text(&String::from_utf8_lossy(&output.stdout)).await
    } else {
        Ok(None)
    }
}

async fn read_image_via_wl_paste(runner: &dyn ClipboardCommandRunner) -> Option<ClipboardImage> {
    let list = runner
        .run(
            "wl-paste",
            &["--list-types".to_owned()],
            DEFAULT_LIST_TIMEOUT,
            None,
        )
        .await;
    let mime = select_preferred_image_mime_type(&parse_target_list(&list.stdout))?;
    let data = runner
        .run(
            "wl-paste",
            &["--type".to_owned(), mime.clone(), "--no-newline".to_owned()],
            DEFAULT_READ_TIMEOUT,
            None,
        )
        .await;
    (data.ok && !data.stdout.is_empty()).then(|| ClipboardImage {
        bytes: data.stdout,
        mime_type: base_mime_type(&mime),
    })
}

async fn xclip_targets(runner: &dyn ClipboardCommandRunner) -> CommandOutput {
    runner
        .run(
            "xclip",
            &[
                "-selection".to_owned(),
                "clipboard".to_owned(),
                "-t".to_owned(),
                "TARGETS".to_owned(),
                "-o".to_owned(),
            ],
            DEFAULT_LIST_TIMEOUT,
            None,
        )
        .await
}

async fn read_file_media_via_xclip(
    runner: &dyn ClipboardCommandRunner,
) -> Result<Option<ClipboardMedia>, ClipboardMediaError> {
    let targets = xclip_targets(runner).await;
    let Some(uri_type) = targets
        .ok
        .then(|| parse_target_list(&targets.stdout))
        .into_iter()
        .flatten()
        .find(|target| base_mime_type(target) == "text/uri-list")
    else {
        return Ok(None);
    };
    let output = runner
        .run(
            "xclip",
            &[
                "-selection".to_owned(),
                "clipboard".to_owned(),
                "-t".to_owned(),
                uri_type,
                "-o".to_owned(),
            ],
            DEFAULT_READ_TIMEOUT,
            None,
        )
        .await;
    if output.ok {
        read_media_from_text(&String::from_utf8_lossy(&output.stdout)).await
    } else {
        Ok(None)
    }
}

async fn read_image_via_xclip(runner: &dyn ClipboardCommandRunner) -> Option<ClipboardImage> {
    let targets = xclip_targets(runner).await;
    let candidates = if targets.ok {
        parse_target_list(&targets.stdout)
    } else {
        Vec::new()
    };
    let mut mime_types = Vec::new();
    if let Some(preferred) = select_preferred_image_mime_type(&candidates) {
        mime_types.push(preferred);
    }
    mime_types.extend(
        SUPPORTED_IMAGE_MIME_TYPES
            .iter()
            .map(|mime| (*mime).to_owned()),
    );
    for mime in mime_types {
        let output = runner
            .run(
                "xclip",
                &[
                    "-selection".to_owned(),
                    "clipboard".to_owned(),
                    "-t".to_owned(),
                    mime.clone(),
                    "-o".to_owned(),
                ],
                DEFAULT_READ_TIMEOUT,
                None,
            )
            .await;
        if output.ok && !output.stdout.is_empty() {
            return Some(ClipboardImage {
                bytes: output.stdout,
                mime_type: base_mime_type(&mime),
            });
        }
    }
    None
}

async fn read_image_via_powershell(
    runner: &dyn ClipboardCommandRunner,
    environment: &HashMap<String, String>,
    wsl: bool,
) -> Option<ClipboardImage> {
    let file = std::env::temp_dir().join(format!("kimi-clip-{}.png", Uuid::new_v4()));
    let windows_path = if wsl {
        let converted = runner
            .run(
                "wslpath",
                &["-w".to_owned(), file.to_string_lossy().into_owned()],
                DEFAULT_LIST_TIMEOUT,
                None,
            )
            .await;
        if !converted.ok {
            return None;
        }
        String::from_utf8_lossy(&converted.stdout).trim().to_owned()
    } else {
        file.to_string_lossy().into_owned()
    };
    if windows_path.is_empty() {
        return None;
    }
    let script = "Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing; $path = $env:KIMI_CLIPBOARD_IMAGE_PATH; $img = [System.Windows.Forms.Clipboard]::GetImage(); if ($img) { $img.Save($path, [System.Drawing.Imaging.ImageFormat]::Png); Write-Output 'ok' } else { Write-Output 'empty' }";
    let mut process_environment = environment.clone();
    process_environment.insert("KIMI_CLIPBOARD_IMAGE_PATH".to_owned(), windows_path);
    let output = runner
        .run(
            "powershell.exe",
            &[
                "-NoProfile".to_owned(),
                "-Command".to_owned(),
                script.to_owned(),
            ],
            DEFAULT_POWERSHELL_TIMEOUT,
            Some(&process_environment),
        )
        .await;
    let result = if output.ok && String::from_utf8_lossy(&output.stdout).trim() == "ok" {
        tokio::fs::read(&file)
            .await
            .ok()
            .filter(|bytes| !bytes.is_empty())
    } else {
        None
    };
    let _ = tokio::fs::remove_file(file).await;
    result.map(|bytes| ClipboardImage {
        bytes,
        mime_type: "image/png".to_owned(),
    })
}

async fn read_macos_file_paths(runner: &dyn ClipboardCommandRunner) -> Vec<PathBuf> {
    let output = runner
        .run(
            "osascript",
            &[
                "-l".to_owned(),
                "JavaScript".to_owned(),
                "-e".to_owned(),
                MACOS_FILE_PATH_SCRIPT.to_owned(),
            ],
            DEFAULT_LIST_TIMEOUT,
            None,
        )
        .await;
    if output.ok {
        parse_clipboard_paths(&String::from_utf8_lossy(&output.stdout))
    } else {
        Vec::new()
    }
}

fn video_mime_from_path(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_string_lossy().to_lowercase().as_str() {
        "mp4" => Some("video/mp4"),
        "mpg" | "mpeg" => Some("video/mpeg"),
        "mkv" => Some("video/x-matroska"),
        "avi" => Some("video/x-msvideo"),
        "mov" => Some("video/quicktime"),
        "ogv" => Some("video/ogg"),
        "wmv" => Some("video/x-ms-wmv"),
        "webm" => Some("video/webm"),
        "m4v" => Some("video/x-m4v"),
        "flv" => Some("video/x-flv"),
        "3gp" => Some("video/3gpp"),
        "3g2" => Some("video/3gpp2"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct Runner {
        outputs: Mutex<Vec<CommandOutput>>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ClipboardCommandRunner for Runner {
        async fn run(
            &self,
            command: &str,
            args: &[String],
            _: Duration,
            _: Option<&HashMap<String, String>>,
        ) -> CommandOutput {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("{command} {}", args.join(" ")));
            self.outputs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(0)
        }
    }

    fn ok(value: impl Into<Vec<u8>>) -> CommandOutput {
        CommandOutput {
            stdout: value.into(),
            ok: true,
        }
    }

    fn png() -> Vec<u8> {
        let mut bytes = vec![0; 24];
        bytes[..8].copy_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        bytes[19] = 1;
        bytes[23] = 1;
        bytes
    }

    #[test]
    fn preferred_mime_uses_pipeline_order_then_any_image() {
        assert_eq!(
            select_preferred_image_mime_type(&[
                "image/gif".to_owned(),
                "image/png; charset=binary".to_owned()
            ])
            .as_deref(),
            Some("image/png; charset=binary")
        );
        assert_eq!(
            select_preferred_image_mime_type(&["image/bmp".to_owned()]).as_deref(),
            Some("image/bmp")
        );
    }

    #[tokio::test]
    async fn parses_file_uri_image_video_and_enforces_video_limit() {
        let root = std::env::temp_dir().join(format!("clipboard-media-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.expect("directory");
        let image_path = root.join("image.png");
        tokio::fs::write(&image_path, png()).await.expect("image");
        let uri = Url::from_file_path(&image_path).expect("uri").to_string();
        assert_eq!(
            parse_clipboard_paths(&format!("# comment\r\n{uri}\0")).len(),
            1
        );
        assert!(matches!(
            read_media_path(&image_path).await.expect("image"),
            Some(ClipboardMedia::Image(_))
        ));

        let video = root.join("clip.MP4");
        tokio::fs::write(&video, b"video").await.expect("video");
        assert!(
            matches!(read_media_path(&video).await.expect("video"), Some(ClipboardMedia::Video(value)) if value.mime_type == "video/mp4")
        );
        let huge = root.join("huge.webm");
        let file = std::fs::File::create(&huge).expect("huge");
        file.set_len(MAX_VIDEO_BYTES + 1).expect("sparse size");
        assert!(read_media_path(&huge).await.is_err());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn wayland_reads_file_uri_before_image_and_termux_skips_all_commands() {
        let root = std::env::temp_dir().join(format!("clipboard-wayland-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.expect("directory");
        let image = root.join("file.png");
        tokio::fs::write(&image, png()).await.expect("image");
        let runner = Runner {
            outputs: Mutex::new(vec![
                ok(b"text/uri-list\n".to_vec()),
                ok(Url::from_file_path(&image).expect("uri").to_string()),
            ]),
            calls: Mutex::new(Vec::new()),
        };
        let media = read_clipboard_media_with(
            &runner,
            ClipboardPlatform::Linux,
            &HashMap::from([("WAYLAND_DISPLAY".to_owned(), "wayland-0".to_owned())]),
            false,
        )
        .await
        .expect("clipboard");
        assert!(matches!(media, Some(ClipboardMedia::Image(_))));
        assert_eq!(
            runner
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            2
        );

        let skipped = Runner {
            outputs: Mutex::new(vec![]),
            calls: Mutex::new(vec![]),
        };
        assert_eq!(
            read_clipboard_media_with(
                &skipped,
                ClipboardPlatform::Linux,
                &HashMap::from([("TERMUX_VERSION".to_owned(), "1".to_owned())]),
                false
            )
            .await
            .expect("termux"),
            None
        );
        assert!(
            skipped
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn xclip_image_fallback_uses_preferred_type_and_rejects_unsupported_result() {
        let runner = Runner {
            outputs: Mutex::new(vec![
                ok(b"image/webp\nimage/png\n".to_vec()),
                ok(b"image/webp\nimage/png\n".to_vec()),
                ok(png()),
            ]),
            calls: Mutex::new(Vec::new()),
        };
        let media =
            read_clipboard_media_with(&runner, ClipboardPlatform::Linux, &HashMap::new(), false)
                .await
                .expect("clipboard");
        assert!(
            matches!(media, Some(ClipboardMedia::Image(image)) if image.mime_type == "image/png")
        );
    }
}
