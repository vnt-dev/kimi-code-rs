//! Read image and video files as model-facing multimodal content.
//!
//! Original: `packages/agent-core-v2/src/agent/media/tools/read-media.ts`.

use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    agent::media::{
        CompressImageOptions, CropImageOptions, CropImageOutcome, DetectFileTypeMode, FileTypeKind,
        IMAGE_BYTE_BUDGET, ImageCompressionTelemetry, ImageCropRegion, MAX_IMAGE_DECODE_BYTES,
        MEDIA_SNIFF_BYTES, build_image_conversion_guidance, compress_image_for_model,
        crop_image_for_model, detect_file_type, format_byte_size, is_model_accepted_image_mime,
        resolve_max_image_edge_px, resolve_read_image_byte_budget, sniff_image_dimensions,
    },
    app::telemetry::TelemetryServiceHandle,
    kosong::contract::{
        capability::ModelCapability,
        message::{ContentPart, MediaUrl},
        tool::Tool,
    },
    os::interface::{
        host_environment::HostEnvironmentHandle, host_file_system::HostFileSystemServiceHandle,
    },
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolOutput, ExecutableToolResult,
        RunnableToolExecution, ToolAccess, ToolExecution,
        input_schema::to_input_json_schema,
        path_access::{
            DEFAULT_WORKSPACE_ACCESS_POLICY, PathAccessOperation, WorkspaceConfig,
            resolve_path_access_path,
        },
        rule_match::{PermissionPathMatchOptions, literal_rule_pattern, matches_path_rule_subject},
    },
};
use kimi_code_protocol::{FileIoOperation, ToolInputDisplay};

pub use crate::kosong::contract::provider::VideoUploadInput;

pub const MAX_MEDIA_MEGABYTES: u64 = 100;
pub const MAX_MEDIA_BYTES: u64 = MAX_MEDIA_MEGABYTES * 1_024 * 1_024;
const READ_MEDIA_DESCRIPTION_HEAD: &str = include_str!("read-media.md");

pub type VideoUploadFuture = BoxFuture<'static, Result<ContentPart, String>>;
pub type VideoUploader = Arc<dyn Fn(VideoUploadInput) -> VideoUploadFuture + Send + Sync + 'static>;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct ReadMediaRegion {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl From<ReadMediaRegion> for ImageCropRegion {
    fn from(region: ReadMediaRegion) -> Self {
        Self {
            x: region.x as f64,
            y: region.y as f64,
            width: region.width as f64,
            height: region.height as f64,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ReadMediaFileInput {
    pub path: String,
    #[serde(default)]
    pub region: Option<ReadMediaRegion>,
    #[serde(default)]
    pub full_resolution: Option<bool>,
}

pub fn read_media_file_parameters() -> Map<String, Value> {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to an image or video file. Relative paths resolve against the working directory; a path outside the working directory must be absolute. Directories and text files are not supported."
                },
                "region": {
                    "type": "object",
                    "properties": {
                        "x": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "Left edge of the crop, in original-image pixels."
                        },
                        "y": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "Top edge of the crop, in original-image pixels."
                        },
                        "width": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Crop width, in original-image pixels."
                        },
                        "height": {
                            "type": "integer",
                            "minimum": 1,
                            "description": "Crop height, in original-image pixels."
                        }
                    },
                    "required": ["x", "y", "width", "height"],
                    "additionalProperties": false,
                    "description": "Images only: view just this rectangle of the image (original-image pixel coordinates). Use after a downsampled full view to inspect fine detail — a region within the size limits is delivered at full fidelity."
                },
                "full_resolution": {
                    "type": "boolean",
                    "description": "Images only: skip the default downscaling and view at native resolution. Fails with an explicit error when the payload would exceed the per-image byte limit; use region for files that large."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
        .as_object()
        .cloned()
        .expect("ReadMediaFile schema is an object"),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImageDeliveryKind {
    Untouched,
    Downsampled,
    Crop,
    Full,
}

#[derive(Clone, Debug, PartialEq)]
struct ImageDelivery {
    kind: ImageDeliveryKind,
    width: i64,
    height: i64,
    byte_length: usize,
    mime_type: String,
    region: Option<ImageCropRegion>,
    resized: Option<bool>,
}

#[derive(Clone, Copy)]
struct Dimensions {
    width: i64,
    height: i64,
}

#[derive(Clone, Copy)]
enum MediaKind {
    Image,
    Video,
}

impl MediaKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
        }
    }
}

#[derive(Clone)]
pub struct ReadMediaFileTool {
    fs: HostFileSystemServiceHandle,
    environment: HostEnvironmentHandle,
    workspace: WorkspaceConfig,
    capabilities: ModelCapability,
    video_uploader: Option<VideoUploader>,
    compress_telemetry: Option<ImageCompressionTelemetry>,
    definition: Tool,
}

impl ReadMediaFileTool {
    pub fn new(
        fs: HostFileSystemServiceHandle,
        environment: HostEnvironmentHandle,
        workspace: WorkspaceConfig,
        capabilities: ModelCapability,
        video_uploader: Option<VideoUploader>,
        telemetry: Option<TelemetryServiceHandle>,
    ) -> Self {
        let definition = Tool {
            name: "ReadMediaFile".into(),
            description: build_description(&capabilities),
            parameters: read_media_file_parameters(),
            deferred: None,
        };
        Self {
            fs,
            environment,
            workspace,
            capabilities,
            video_uploader,
            compress_telemetry: telemetry.map(|client| ImageCompressionTelemetry {
                client,
                source: "read_media".into(),
            }),
            definition,
        }
    }

    async fn execute(&self, args: ReadMediaFileInput, safe_path: String) -> ExecutableToolResult {
        if args.path.is_empty() {
            return ExecutableToolResult::error("File path cannot be empty.");
        }
        match self.execute_inner(&args, &safe_path).await {
            Ok(result) => result,
            Err(error) => {
                ExecutableToolResult::error(format!("Failed to read {}: {error}", args.path))
            }
        }
    }

    async fn execute_inner(
        &self,
        args: &ReadMediaFileInput,
        safe_path: &str,
    ) -> Result<ExecutableToolResult, String> {
        let header = self
            .fs
            .read_bytes(Path::new(safe_path), Some(MEDIA_SNIFF_BYTES))
            .await
            .map_err(|error| error.to_string())?;
        let file_type = detect_file_type(safe_path, Some(&header), DetectFileTypeMode::Media);

        match file_type.kind {
            FileTypeKind::Text => {
                return Ok(ExecutableToolResult::error(format!(
                    "\"{}\" is a text file. Use Read to read text files.",
                    args.path
                )));
            }
            FileTypeKind::Unknown => {
                return Ok(ExecutableToolResult::error(format!(
                    "\"{}\" is not a supported image or video file. Use Read for text files, or Bash or an MCP tool for other binary formats.",
                    args.path
                )));
            }
            FileTypeKind::Image if !self.capabilities.image_in => {
                return Ok(ExecutableToolResult::error(
                    "The current model does not support image input. Tell the user to use a model with image input capability.",
                ));
            }
            FileTypeKind::Image if !is_model_accepted_image_mime(&file_type.mime_type) => {
                let info = self.environment.info().map_err(|error| error.to_string())?;
                return Ok(ExecutableToolResult::error(
                    build_image_conversion_guidance(
                        &args.path,
                        &file_type.mime_type,
                        &info.os_kind,
                    ),
                ));
            }
            FileTypeKind::Video if !self.capabilities.video_in => {
                return Ok(ExecutableToolResult::error(
                    "The current model does not support video input. Tell the user to use a model with video input capability.",
                ));
            }
            _ => {}
        }

        let stat = self
            .fs
            .stat(Path::new(safe_path))
            .await
            .map_err(|error| error.to_string())?;
        if stat.size == 0 {
            return Ok(ExecutableToolResult::error(format!(
                "\"{}\" is empty.",
                args.path
            )));
        }
        if stat.size > MAX_MEDIA_BYTES {
            return Ok(ExecutableToolResult::error(format!(
                "\"{}\" is {} bytes, which exceeds the maximum {MAX_MEDIA_MEGABYTES}MB for media files.",
                args.path, stat.size
            )));
        }

        let is_image = file_type.kind == FileTypeKind::Image;
        if !is_image && (args.region.is_some() || args.full_resolution == Some(true)) {
            return Ok(ExecutableToolResult::error(
                "region and full_resolution apply only to image files.",
            ));
        }
        if is_image
            && stat.size > MAX_IMAGE_DECODE_BYTES as u64
            && (args.region.is_some() || args.full_resolution == Some(true))
        {
            return Ok(ExecutableToolResult::error(build_image_decode_limit_error(
                stat.size,
            )));
        }
        if is_image
            && args.region.is_none()
            && args.full_resolution == Some(true)
            && stat.size > IMAGE_BYTE_BUDGET as u64
        {
            return Ok(ExecutableToolResult::error(
                build_full_resolution_limit_error(&args.path, stat.size),
            ));
        }

        let read_byte_budget = resolve_read_image_byte_budget();
        let max_edge = resolve_max_image_edge_px();
        if is_image
            && args.region.is_none()
            && args.full_resolution != Some(true)
            && stat.size > MAX_IMAGE_DECODE_BYTES as u64
            && stat.size > read_byte_budget as u64
        {
            return Ok(ExecutableToolResult::error(
                build_image_delivery_limit_error(stat.size, read_byte_budget, max_edge),
            ));
        }

        let data = self
            .fs
            .read_bytes(Path::new(safe_path), None)
            .await
            .map_err(|error| error.to_string())?;
        let mut dimensions = is_image
            .then(|| sniff_image_dimensions(&data))
            .flatten()
            .map(|value| Dimensions {
                width: value.width,
                height: value.height,
            });
        let mut delivery = None;

        let media_part = if is_image {
            if let Some(region) = args.region {
                let outcome = crop_image_for_model(
                    &data,
                    &file_type.mime_type,
                    region.into(),
                    &CropImageOptions {
                        compress: CompressImageOptions {
                            telemetry: self.compress_telemetry.clone(),
                            ..CompressImageOptions::default()
                        },
                        skip_resize: args.full_resolution == Some(true),
                    },
                );
                let CropImageOutcome::Success(outcome) = outcome else {
                    let CropImageOutcome::Failure(failure) = outcome else {
                        unreachable!()
                    };
                    return Ok(ExecutableToolResult::error(format!(
                        "Cannot read region from \"{}\": {}",
                        args.path, failure.error
                    )));
                };
                dimensions = Some(Dimensions {
                    width: outcome.original_width,
                    height: outcome.original_height,
                });
                delivery = Some(ImageDelivery {
                    kind: ImageDeliveryKind::Crop,
                    width: outcome.width,
                    height: outcome.height,
                    byte_length: outcome.final_byte_length,
                    mime_type: outcome.mime_type.clone(),
                    region: Some(outcome.region),
                    resized: Some(outcome.resized),
                });
                image_part(&outcome.data, &outcome.mime_type)
            } else if args.full_resolution == Some(true) {
                if data.len() > IMAGE_BYTE_BUDGET {
                    return Ok(ExecutableToolResult::error(
                        build_full_resolution_limit_error(&args.path, data.len() as u64),
                    ));
                }
                delivery = Some(ImageDelivery {
                    kind: ImageDeliveryKind::Full,
                    width: dimensions.map_or(0, |value| value.width),
                    height: dimensions.map_or(0, |value| value.height),
                    byte_length: data.len(),
                    mime_type: file_type.mime_type.clone(),
                    region: None,
                    resized: None,
                });
                image_part(&data, &file_type.mime_type)
            } else {
                let compressed = compress_image_for_model(
                    &data,
                    &file_type.mime_type,
                    &CompressImageOptions {
                        max_edge: Some(max_edge),
                        byte_budget: Some(read_byte_budget),
                        telemetry: self.compress_telemetry.clone(),
                        ..CompressImageOptions::default()
                    },
                );
                if compressed.final_byte_length > read_byte_budget
                    || compressed.width.max(compressed.height) > i64::from(max_edge)
                {
                    return Ok(ExecutableToolResult::error(
                        build_image_delivery_limit_error(
                            compressed.final_byte_length as u64,
                            read_byte_budget,
                            max_edge,
                        ),
                    ));
                }
                if compressed.changed {
                    dimensions = Some(Dimensions {
                        width: compressed.original_width,
                        height: compressed.original_height,
                    });
                }
                delivery = Some(ImageDelivery {
                    kind: if compressed.changed {
                        ImageDeliveryKind::Downsampled
                    } else {
                        ImageDeliveryKind::Untouched
                    },
                    width: compressed.width,
                    height: compressed.height,
                    byte_length: compressed.final_byte_length,
                    mime_type: compressed.mime_type.clone(),
                    region: None,
                    resized: None,
                });
                image_part(&compressed.data, &compressed.mime_type)
            }
        } else if let Some(uploader) = &self.video_uploader {
            uploader(VideoUploadInput {
                data,
                mime_type: file_type.mime_type.clone(),
                filename: media_filename(safe_path),
            })
            .await?
        } else {
            video_part(&data, &file_type.mime_type)
        };

        let kind = if is_image {
            MediaKind::Image
        } else {
            MediaKind::Video
        };
        let tag = kind.as_str();
        let output = vec![
            ContentPart::Text {
                text: format!("<{tag} path=\"{safe_path}\">"),
            },
            media_part,
            ContentPart::Text {
                text: format!("</{tag}>"),
            },
        ];
        let note = build_media_note(
            kind,
            &file_type.mime_type,
            stat.size,
            dimensions,
            delivery.as_ref(),
        );
        Ok(ExecutableToolResult {
            output: ExecutableToolOutput::Content(output),
            is_error: false,
            stop_turn: None,
            truncated: None,
            note: Some(note),
            delivery: None,
        })
    }
}

#[async_trait]
impl ExecutableTool for ReadMediaFileTool {
    type Input = ReadMediaFileInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, args: Self::Input) -> ToolExecution {
        if args.path.is_empty() {
            return ToolExecution::Error(ExecutableToolResult::error("File path cannot be empty."));
        }
        let info = match self.environment.info() {
            Ok(info) => info,
            Err(error) => {
                return ToolExecution::Error(ExecutableToolResult::error(error.to_string()));
            }
        };
        let safe_path = match resolve_path_access_path(
            &args.path,
            &info,
            &self.workspace,
            PathAccessOperation::Read,
            DEFAULT_WORKSPACE_ACCESS_POLICY,
            true,
        ) {
            Ok(path) => path,
            Err(error) => {
                return ToolExecution::Error(ExecutableToolResult::error(error.to_string()));
            }
        };

        let rule_path = safe_path.clone();
        let rule_cwd = self.workspace.workspace_dir.clone();
        let rule_home = info.home_dir;
        let path_class = info.path_class;
        let tool = self.clone();
        let execution_args = args.clone();
        let execution_path = safe_path.clone();
        let mut execution = RunnableToolExecution::new(
            literal_rule_pattern("ReadMediaFile", &safe_path),
            Arc::new(move |_context: ExecutableToolContext| {
                let tool = tool.clone();
                let args = execution_args.clone();
                let path = execution_path.clone();
                Box::pin(async move { tool.execute(args, path).await })
                    as BoxFuture<'static, ExecutableToolResult>
            }),
        );
        execution.accesses = Some(ToolAccess::read_file(safe_path.clone()));
        execution.description = Some(format!("Reading media: {}", args.path));
        execution.display = Some(ToolInputDisplay::FileIo {
            operation: FileIoOperation::Read,
            path: safe_path,
            detail: None,
            content: None,
            before: None,
            after: None,
        });
        execution.matches_rule = Some(Arc::new(move |rule_args| {
            matches_path_rule_subject(
                rule_args,
                &rule_path,
                PermissionPathMatchOptions {
                    cwd: Some(&rule_cwd),
                    path_class: Some(path_class),
                    home_dir: Some(&rule_home),
                    case_insensitive_paths: None,
                },
            )
        }));
        ToolExecution::Runnable(execution)
    }
}

pub fn build_description(capabilities: &ModelCapability) -> String {
    let head = READ_MEDIA_DESCRIPTION_HEAD
        .replace("${MAX_MEDIA_MEGABYTES}", &MAX_MEDIA_MEGABYTES.to_string());
    let capability_text = match (capabilities.image_in, capabilities.video_in) {
        (true, true) => "- This tool supports image and video files for the current model.".into(),
        (true, false) => [
            "- This tool supports image files for the current model.",
            "- Video files are not supported by the current model.",
        ]
        .join("\n"),
        (false, true) => [
            "- This tool supports video files for the current model.",
            "- Image files are not supported by the current model.",
        ]
        .join("\n"),
        (false, false) => "- The current model does not support image or video input.".into(),
    };
    format!("{head}\n{capability_text}")
}

fn image_part(data: &[u8], mime_type: &str) -> ContentPart {
    ContentPart::ImageUrl {
        image_url: MediaUrl {
            url: format!("data:{mime_type};base64,{}", STANDARD.encode(data)),
            id: None,
        },
    }
}

fn video_part(data: &[u8], mime_type: &str) -> ContentPart {
    ContentPart::VideoUrl {
        video_url: MediaUrl {
            url: format!("data:{mime_type};base64,{}", STANDARD.encode(data)),
            id: None,
        },
    }
}

fn media_filename(path: &str) -> Option<String> {
    path.rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

fn build_media_note(
    kind: MediaKind,
    mime_type: &str,
    byte_size: u64,
    dimensions: Option<Dimensions>,
    delivery: Option<&ImageDelivery>,
) -> String {
    let mut parts = vec![
        format!("Read {} file.", kind.as_str()),
        format!("Mime type: {mime_type}."),
        format!("Size: {byte_size} bytes."),
    ];
    if matches!(kind, MediaKind::Image)
        && let Some(dimensions) = dimensions
    {
        parts.push(format!(
            "Original dimensions: {}x{} pixels.",
            dimensions.width, dimensions.height
        ));
    }
    match delivery {
        Some(delivery) if delivery.kind == ImageDeliveryKind::Downsampled => {
            parts.push(format!(
                "The attached image was downsampled to {}x{} pixels ({}, {}) to fit model limits; fine detail may be lost.",
                delivery.width,
                delivery.height,
                delivery.mime_type,
                format_byte_size(delivery.byte_length as f64)
            ));
            parts.push(
                "To inspect fine detail, call ReadMediaFile again with the region parameter (original-image pixel coordinates) to view a crop at full fidelity.".into(),
            );
        }
        Some(delivery) if delivery.kind == ImageDeliveryKind::Crop && delivery.region.is_some() => {
            let region = delivery.region.expect("crop delivery has a region");
            let resolution = if delivery.resized == Some(true) {
                format!(
                    ", downsampled to {}x{} pixels",
                    delivery.width, delivery.height
                )
            } else {
                " at native resolution".into()
            };
            parts.push(format!(
                "Showing region (x={}, y={}, width={}, height={}) of the original image{resolution}.",
                number(region.x),
                number(region.y),
                number(region.width),
                number(region.height)
            ));
            parts.push(format!(
                "To output coordinates in original-image pixels, locate them within this crop and add the region offset (x={}, y={}).",
                number(region.x),
                number(region.y)
            ));
        }
        Some(delivery) if delivery.kind == ImageDeliveryKind::Full => {
            parts.push("Shown at native resolution; no downscaling applied.".into());
        }
        _ => {}
    }
    if matches!(kind, MediaKind::Image)
        && dimensions.is_some()
        && delivery.is_none_or(|value| value.kind != ImageDeliveryKind::Crop)
    {
        parts.push(
            "If you need to output coordinates, output relative coordinates first and compute absolute coordinates using the original image size.".into(),
        );
    }
    parts.push(
        "If you generate or edit images or videos via commands or scripts, read the result back immediately before continuing.".into(),
    );
    format!("<system>{}</system>", parts.join(" "))
}

fn build_image_delivery_limit_error(
    final_bytes: u64,
    read_byte_budget: usize,
    max_edge: u32,
) -> String {
    format!(
        "Image is too large to send safely after compression ({final_bytes} bytes; limit {read_byte_budget} bytes and {max_edge}px on the longest edge). The original image was not sent to the model. Do not retry the same file unchanged. Use Bash or an available image-processing tool to create a smaller copy within both limits, then call ReadMediaFile on the smaller copy."
    )
}

fn build_image_decode_limit_error(final_bytes: u64) -> String {
    format!(
        "Image is too large to process safely for region or full_resolution ({final_bytes} bytes; safe decode limit {MAX_IMAGE_DECODE_BYTES} bytes). The original image was not sent to the model. Do not retry the same file unchanged. Use Bash or an available image-processing tool to create a smaller copy or crop the needed region into a separate image, then call ReadMediaFile on the resulting file."
    )
}

fn build_full_resolution_limit_error(path: &str, final_bytes: u64) -> String {
    format!(
        "\"{path}\" is {final_bytes} bytes ({}), over the {IMAGE_BYTE_BUDGET}-byte ({}) per-image limit, so full_resolution cannot be honored. Use region to view a crop at full fidelity instead.",
        format_byte_size(final_bytes as f64),
        format_byte_size(IMAGE_BYTE_BUDGET as f64)
    )
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Cursor,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use futures_util::stream;
    use image::{DynamicImage, ImageFormat, RgbaImage};

    use crate::{
        _base::{
            errors::errors::BugIndicatingError,
            exec_env::environment_probe::{
                HostEnvironmentInfo, HostEnvironmentProbeError, ShellName,
            },
        },
        kosong::contract::capability::UNKNOWN_CAPABILITY,
        os::interface::{
            host_environment::{HostEnvironment, PathClass},
            host_file_system::{
                HostDirEntry, HostFileStat, HostFileSystemService, HostLineStream, ReadTextOptions,
            },
            host_fs_errors::HostFsError,
        },
    };

    struct MemoryFs {
        data: Vec<u8>,
        stat_size: u64,
        full_reads: AtomicUsize,
    }

    #[async_trait]
    impl HostFileSystemService for MemoryFs {
        async fn read_text(
            &self,
            _path: &Path,
            _options: Option<ReadTextOptions>,
        ) -> Result<String, HostFsError> {
            unreachable!()
        }

        async fn write_text(&self, _path: &Path, _data: &str) -> Result<(), HostFsError> {
            unreachable!()
        }

        async fn append_text(&self, _path: &Path, _data: &str) -> Result<(), HostFsError> {
            unreachable!()
        }

        async fn read_bytes(
            &self,
            _path: &Path,
            count: Option<usize>,
        ) -> Result<Vec<u8>, HostFsError> {
            if let Some(count) = count {
                Ok(self.data[..self.data.len().min(count)].to_vec())
            } else {
                self.full_reads.fetch_add(1, Ordering::SeqCst);
                Ok(self.data.clone())
            }
        }

        async fn write_bytes(&self, _path: &Path, _data: &[u8]) -> Result<(), HostFsError> {
            unreachable!()
        }

        fn read_lines(&self, _path: &Path, _options: Option<ReadTextOptions>) -> HostLineStream {
            Box::pin(stream::empty())
        }

        async fn create_exclusive(&self, _path: &Path, _data: &[u8]) -> Result<bool, HostFsError> {
            unreachable!()
        }

        async fn stat(&self, _path: &Path) -> Result<HostFileStat, HostFsError> {
            Ok(HostFileStat {
                is_file: true,
                is_directory: false,
                is_symbolic_link: false,
                size: self.stat_size,
                modified_millis: None,
                inode: None,
            })
        }

        async fn lstat(&self, _path: &Path) -> Result<HostFileStat, HostFsError> {
            unreachable!()
        }

        async fn read_dir(&self, _path: &Path) -> Result<Vec<HostDirEntry>, HostFsError> {
            unreachable!()
        }

        async fn create_dir(&self, _path: &Path, _recursive: bool) -> Result<(), HostFsError> {
            unreachable!()
        }

        async fn remove(&self, _path: &Path) -> Result<(), HostFsError> {
            unreachable!()
        }

        async fn real_path(&self, _path: &Path) -> Result<String, HostFsError> {
            unreachable!()
        }
    }

    struct TestEnvironment;

    #[async_trait]
    impl HostEnvironment for TestEnvironment {
        async fn ready(&self) -> Result<(), HostEnvironmentProbeError> {
            Ok(())
        }

        fn info(&self) -> Result<HostEnvironmentInfo, BugIndicatingError> {
            Ok(HostEnvironmentInfo {
                os_kind: "Linux".into(),
                os_arch: "x64".into(),
                os_version: "test".into(),
                shell_name: ShellName::Bash,
                shell_path: "/bin/bash".into(),
                path_class: PathClass::Posix,
                home_dir: "/home/test".into(),
            })
        }
    }

    fn png_bytes() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 1, image::Rgba([1, 2, 3, 255])))
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    fn test_tool(fs: Arc<MemoryFs>, capabilities: ModelCapability) -> ReadMediaFileTool {
        ReadMediaFileTool::new(
            HostFileSystemServiceHandle(fs),
            HostEnvironmentHandle(Arc::new(TestEnvironment)),
            WorkspaceConfig {
                workspace_dir: "/work".into(),
                additional_dirs: Vec::new(),
            },
            capabilities,
            None,
            None,
        )
    }

    #[test]
    fn description_is_capability_aware_and_renders_limit() {
        let both = ModelCapability {
            image_in: true,
            video_in: true,
            ..UNKNOWN_CAPABILITY
        };
        let description = build_description(&both);
        assert!(description.contains("maximum size that can be read is 100MB"));
        assert!(
            description
                .ends_with("- This tool supports image and video files for the current model.")
        );

        let image = ModelCapability {
            image_in: true,
            ..UNKNOWN_CAPABILITY
        };
        let description = build_description(&image);
        assert!(description.contains("supports image files"));
        assert!(description.contains("Video files are not supported"));
    }

    #[test]
    fn input_schema_preserves_region_and_native_read_constraints() {
        let schema = Value::Object(read_media_file_parameters());
        assert_eq!(schema["required"], json!(["path"]));
        assert_eq!(
            schema["properties"]["region"]["properties"]["x"]["minimum"],
            0
        );
        assert_eq!(
            schema["properties"]["region"]["properties"]["width"]["minimum"],
            1
        );
        assert_eq!(schema["properties"]["full_resolution"]["type"], "boolean");
    }

    #[test]
    fn note_matches_downsample_crop_and_full_delivery_wording() {
        let dimensions = Some(Dimensions {
            width: 4000,
            height: 3000,
        });
        let downsampled = ImageDelivery {
            kind: ImageDeliveryKind::Downsampled,
            width: 1000,
            height: 750,
            byte_length: 2048,
            mime_type: "image/jpeg".into(),
            region: None,
            resized: None,
        };
        let note = build_media_note(
            MediaKind::Image,
            "image/png",
            4096,
            dimensions,
            Some(&downsampled),
        );
        assert!(note.starts_with("<system>Read image file."));
        assert!(note.contains("Original dimensions: 4000x3000 pixels."));
        assert!(note.contains("downsampled to 1000x750 pixels (image/jpeg, 2 KB)"));
        assert!(note.contains("region parameter"));

        let crop = ImageDelivery {
            kind: ImageDeliveryKind::Crop,
            width: 200,
            height: 100,
            byte_length: 100,
            mime_type: "image/png".into(),
            region: Some(ImageCropRegion {
                x: 12.0,
                y: 34.0,
                width: 200.0,
                height: 100.0,
            }),
            resized: Some(false),
        };
        let note = build_media_note(MediaKind::Image, "image/png", 4096, dimensions, Some(&crop));
        assert!(note.contains("at native resolution"));
        assert!(note.contains("region offset (x=12, y=34)"));
        assert!(!note.contains("output relative coordinates first"));
    }

    #[test]
    fn limit_errors_preserve_source_guidance() {
        assert_eq!(
            build_full_resolution_limit_error("a.png", 4_000_000),
            "\"a.png\" is 4000000 bytes (3.8 MB), over the 3932160-byte (3.8 MB) per-image limit, so full_resolution cannot be honored. Use region to view a crop at full fidelity instead."
        );
        assert!(
            build_image_decode_limit_error(70_000_000).contains("safe decode limit 67108864 bytes")
        );
        assert!(
            build_image_delivery_limit_error(500_000, 262_144, 2000)
                .contains("limit 262144 bytes and 2000px")
        );
        assert_eq!(
            media_filename(r"C:\work\clip.mp4").as_deref(),
            Some("clip.mp4")
        );
        assert_eq!(
            media_filename("/work/clip.mp4").as_deref(),
            Some("clip.mp4")
        );
    }

    #[tokio::test]
    async fn reads_supported_image_as_three_content_parts_with_hidden_note() {
        let data = png_bytes();
        let fs = Arc::new(MemoryFs {
            stat_size: data.len() as u64,
            data,
            full_reads: AtomicUsize::new(0),
        });
        let tool = test_tool(
            fs.clone(),
            ModelCapability {
                image_in: true,
                ..UNKNOWN_CAPABILITY
            },
        );
        let result = tool
            .execute_inner(
                &ReadMediaFileInput {
                    path: "a.png".into(),
                    region: None,
                    full_resolution: None,
                },
                "/work/a.png",
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        let ExecutableToolOutput::Content(parts) = result.output else {
            panic!("media output must be structured content");
        };
        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts[0],
            ContentPart::Text {
                text: "<image path=\"/work/a.png\">".into()
            }
        );
        assert!(matches!(parts[1], ContentPart::ImageUrl { .. }));
        assert_eq!(
            parts[2],
            ContentPart::Text {
                text: "</image>".into()
            }
        );
        let note = result.note.unwrap();
        assert!(note.contains("Original dimensions: 2x1 pixels."));
        assert_eq!(fs.full_reads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn explicit_native_read_rejects_oversized_source_before_full_read() {
        let data = png_bytes();
        let fs = Arc::new(MemoryFs {
            stat_size: MAX_IMAGE_DECODE_BYTES as u64 + 1,
            data,
            full_reads: AtomicUsize::new(0),
        });
        let tool = test_tool(
            fs.clone(),
            ModelCapability {
                image_in: true,
                ..UNKNOWN_CAPABILITY
            },
        );
        let result = tool
            .execute_inner(
                &ReadMediaFileInput {
                    path: "huge.png".into(),
                    region: None,
                    full_resolution: Some(true),
                },
                "/work/huge.png",
            )
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(
            matches!(result.output, ExecutableToolOutput::Text(ref text) if text.contains("safe decode limit"))
        );
        assert_eq!(fs.full_reads.load(Ordering::SeqCst), 0);
    }
}
