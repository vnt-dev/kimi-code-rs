//! Capability-gated media tool registration and video-uploader binding.
//!
//! Original: `packages/agent-core-v2/src/agent/media/registerMediaTools.ts`.

use std::{
    error::Error,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    _base::{
        di::lifecycle::{DisposableHandle, disposable_none},
        errors::errors::Error2,
        utils::abort::AbortError,
    },
    agent::{
        media::tools::{ReadMediaFileTool, VideoUploadError, VideoUploader},
        tool_registry::{AgentToolRegistryServiceHandle, ToolRegistrationOptions},
    },
    app::telemetry::{
        TelemetryServiceEventExt, TelemetryServiceHandle, VideoUploadEvent, VideoUploadOutcome,
    },
    kosong::{
        contract::{
            capability::ModelCapability, errors::ChatProviderError, provider::VideoUploadSource,
        },
        model::ModelRequester,
    },
    os::interface::{
        host_environment::HostEnvironmentHandle, host_file_system::HostFileSystemServiceHandle,
    },
    tool::{ErasedExecutableTool, path_access::WorkspaceConfig},
};

#[derive(Clone)]
pub struct RegisterMediaToolsDeps {
    pub fs: HostFileSystemServiceHandle,
    pub environment: HostEnvironmentHandle,
    pub workspace: WorkspaceConfig,
    pub capabilities: ModelCapability,
    pub video_uploader: Option<VideoUploader>,
    pub telemetry: Option<TelemetryServiceHandle>,
}

pub fn register_media_tools(
    tool_registry: &AgentToolRegistryServiceHandle,
    dependencies: RegisterMediaToolsDeps,
) -> DisposableHandle {
    if !dependencies.capabilities.image_in && !dependencies.capabilities.video_in {
        return disposable_none();
    }
    let tool: Arc<dyn ErasedExecutableTool> = Arc::new(ReadMediaFileTool::new(
        dependencies.fs,
        dependencies.environment,
        dependencies.workspace,
        dependencies.capabilities,
        dependencies.video_uploader,
        dependencies.telemetry,
    ));
    tool_registry.register(tool, ToolRegistrationOptions::default())
}

#[derive(Clone, Default)]
pub struct VideoUploadTelemetryProps {
    pub model: Option<String>,
    pub provider_type: Option<String>,
    pub protocol: Option<String>,
}

#[derive(Clone)]
pub struct VideoUploadTelemetry {
    pub client: TelemetryServiceHandle,
    pub props: VideoUploadTelemetryProps,
}

pub fn create_video_uploader(
    requester: Option<Arc<dyn ModelRequester>>,
    telemetry: Option<VideoUploadTelemetry>,
) -> Option<VideoUploader> {
    let requester = requester?;
    Some(Arc::new(move |input| {
        let requester = Arc::clone(&requester);
        let telemetry = telemetry.clone();
        Box::pin(async move {
            let started_at = Instant::now();
            let mime_type = input.mime_type.clone();
            let size_bytes = u64::try_from(input.data.len()).unwrap_or(u64::MAX);
            let result = requester
                .upload_video(VideoUploadSource::Data(input), None)
                .await;
            match result {
                Ok(Some(part)) => {
                    track_video_upload(
                        telemetry.as_ref(),
                        mime_type,
                        size_bytes,
                        VideoUploadOutcome::Success,
                        started_at.elapsed(),
                        None,
                    );
                    Ok(part)
                }
                Ok(None) => {
                    track_video_upload(
                        telemetry.as_ref(),
                        mime_type,
                        size_bytes,
                        VideoUploadOutcome::Error,
                        started_at.elapsed(),
                        Some("Error".into()),
                    );
                    Err(VideoUploadError::Unsupported)
                }
                Err(error) => {
                    let error_type = error_name(error.as_ref());
                    track_video_upload(
                        telemetry.as_ref(),
                        mime_type,
                        size_bytes,
                        VideoUploadOutcome::Error,
                        started_at.elapsed(),
                        Some(error_type),
                    );
                    Err(VideoUploadError::Provider(error))
                }
            }
        })
    }))
}

fn track_video_upload(
    telemetry: Option<&VideoUploadTelemetry>,
    mime_type: String,
    size_bytes: u64,
    outcome: VideoUploadOutcome,
    duration: Duration,
    error_type: Option<String>,
) {
    let Some(telemetry) = telemetry else {
        return;
    };
    let event = VideoUploadEvent {
        model: telemetry.props.model.clone(),
        provider_type: telemetry.props.provider_type.clone(),
        protocol: telemetry.props.protocol.clone(),
        mime_type,
        size_bytes,
        outcome,
        duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        error_type,
    };
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let _ = telemetry.client.track_event(&event);
    }));
}

fn error_name(error: &(dyn Error + 'static)) -> String {
    if let Some(error) = error.downcast_ref::<Error2>() {
        return error.name.clone();
    }
    if let Some(error) = error.downcast_ref::<ChatProviderError>() {
        return error.name().into();
    }
    if let Some(error) = error.downcast_ref::<AbortError>() {
        return error.name().into();
    }
    "Error".into()
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use async_trait::async_trait;
    use futures_util::{StreamExt, stream};

    use crate::{
        _base::exec_env::environment_probe::HostEnvironmentProbeError,
        agent::tool_registry::{
            AgentToolRegistryService, AgentToolRegistryServiceContract,
            AgentToolRegistryServiceHandle,
        },
        app::telemetry::{
            TelemetryAppender, TelemetryProperties, TelemetryService, TelemetryServiceContract,
        },
        kosong::{
            contract::{
                message::{ContentPart, MediaUrl},
                provider::{ProviderError, VideoUploadInput},
            },
            model::{ModelRequestInput, ModelRequestParams, ModelRequestStream},
        },
        os::{
            backends::node_local::host_fs_service::HostFileSystem,
            interface::{
                host_environment::{HostEnvironment, HostEnvironmentInfo},
                host_file_system::HostFileSystemService,
            },
        },
    };

    use super::*;

    #[derive(Default)]
    struct TestEnvironment;

    #[async_trait]
    impl HostEnvironment for TestEnvironment {
        async fn ready(&self) -> Result<(), HostEnvironmentProbeError> {
            Ok(())
        }

        fn info(
            &self,
        ) -> Result<HostEnvironmentInfo, crate::_base::errors::errors::BugIndicatingError> {
            unreachable!("media registration does not inspect the environment")
        }
    }

    struct UploadRequester {
        fail: bool,
    }

    impl ModelRequester for UploadRequester {
        fn model(&self) -> Arc<crate::kosong::model::Model> {
            unreachable!("uploader binding does not inspect the model")
        }

        fn request(
            &self,
            _: ModelRequestInput,
            _: Option<tokio_util::sync::CancellationToken>,
            _: Option<ModelRequestParams>,
        ) -> ModelRequestStream {
            stream::empty().boxed()
        }

        fn upload_video(
            &self,
            _: VideoUploadSource,
            _: Option<tokio_util::sync::CancellationToken>,
        ) -> crate::kosong::model::UploadVideoFuture {
            let fail = self.fail;
            Box::pin(async move {
                if fail {
                    Err(Box::new(ChatProviderError::ChatProvider {
                        message: "upload failed".into(),
                    }) as ProviderError)
                } else {
                    Ok(Some(ContentPart::VideoUrl {
                        video_url: MediaUrl {
                            url: "https://video.test/one".into(),
                            id: None,
                        },
                    }))
                }
            })
        }
    }

    #[derive(Default)]
    struct Capture(Mutex<Vec<(String, TelemetryProperties)>>);

    #[async_trait]
    impl TelemetryAppender for Capture {
        fn track(&self, event: &str, properties: Option<&TelemetryProperties>) {
            self.0
                .lock()
                .push((event.into(), properties.cloned().unwrap_or_default()));
        }
    }

    fn capabilities(image_in: bool, video_in: bool) -> ModelCapability {
        ModelCapability {
            image_in,
            video_in,
            audio_in: false,
            thinking: false,
            tool_use: true,
            max_context_tokens: 128_000,
            dynamically_loaded_tools: None,
        }
    }

    fn registration_dependencies(capabilities: ModelCapability) -> RegisterMediaToolsDeps {
        let fs: Arc<dyn HostFileSystemService> = Arc::new(HostFileSystem);
        let environment: Arc<dyn HostEnvironment> = Arc::new(TestEnvironment);
        RegisterMediaToolsDeps {
            fs: HostFileSystemServiceHandle(fs),
            environment: HostEnvironmentHandle(environment),
            workspace: WorkspaceConfig {
                workspace_dir: "/workspace".into(),
                additional_dirs: Vec::new(),
            },
            capabilities,
            video_uploader: None,
            telemetry: None,
        }
    }

    #[test]
    fn registration_is_capability_gated_and_disposable() {
        let registry: Arc<dyn AgentToolRegistryServiceContract> =
            Arc::new(AgentToolRegistryService::new());
        let registry = AgentToolRegistryServiceHandle(registry);

        let empty = register_media_tools(
            &registry,
            registration_dependencies(capabilities(false, false)),
        );
        assert!(registry.resolve("ReadMediaFile").is_none());
        empty.dispose().unwrap();

        let registration = register_media_tools(
            &registry,
            registration_dependencies(capabilities(true, false)),
        );
        let tool = registry.resolve("ReadMediaFile").unwrap();
        assert!(
            tool.tool()
                .description
                .contains("Video files are not supported")
        );
        registration.dispose().unwrap();
        assert!(registry.resolve("ReadMediaFile").is_none());
    }

    #[tokio::test]
    async fn uploader_preserves_results_errors_and_emits_typed_telemetry() {
        assert!(create_video_uploader(None, None).is_none());
        let telemetry_service = Arc::new(TelemetryService::new());
        let capture = Arc::new(Capture::default());
        let appender: Arc<dyn TelemetryAppender> = capture.clone();
        telemetry_service.set_appender(appender);
        let telemetry: Arc<dyn TelemetryServiceContract> = telemetry_service;
        let telemetry = VideoUploadTelemetry {
            client: TelemetryServiceHandle(telemetry),
            props: VideoUploadTelemetryProps {
                model: Some("vision".into()),
                provider_type: Some("kimi".into()),
                protocol: Some("openai".into()),
            },
        };

        let uploader = create_video_uploader(
            Some(Arc::new(UploadRequester { fail: false })),
            Some(telemetry.clone()),
        )
        .unwrap();
        let part = uploader(VideoUploadInput {
            data: vec![1, 2, 3],
            mime_type: "video/mp4".into(),
            filename: Some("one.mp4".into()),
        })
        .await
        .unwrap();
        assert!(matches!(part, ContentPart::VideoUrl { .. }));

        let uploader = create_video_uploader(
            Some(Arc::new(UploadRequester { fail: true })),
            Some(telemetry),
        )
        .unwrap();
        assert_eq!(
            uploader(VideoUploadInput {
                data: vec![4, 5],
                mime_type: "video/webm".into(),
                filename: None,
            })
            .await
            .unwrap_err()
            .to_string(),
            "upload failed"
        );

        let events = capture.0.lock();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "video_upload");
        assert_eq!(
            events[0].1.get("outcome").and_then(|value| value.as_ref()),
            Some(&serde_json::json!("success"))
        );
        assert_eq!(
            events[1]
                .1
                .get("error_type")
                .and_then(|value| value.as_ref()),
            Some(&serde_json::json!("ChatProviderError"))
        );
    }
}
