//! Agent-scope production registrar that follows the bound model.
//!
//! Original: `packages/agent-core-v2/src/agent/media/mediaToolsRegistrar.ts`.

use parking_lot::Mutex;
use std::sync::{Arc, Weak};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            errors::DiError,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableHandle, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::unexpected_error::on_unexpected_error,
    },
    agent::{
        media::{
            AGENT_MEDIA_TOOLS_REGISTRAR_ID, AgentMediaToolsRegistrarContract,
            AgentMediaToolsRegistrarHandle, RegisterMediaToolsDeps, VideoUploadTelemetry,
            VideoUploadTelemetryProps, create_video_uploader, register_media_tools,
        },
        profile::{AGENT_PROFILE_SERVICE_ID, AgentProfileServiceHandle},
        tool_registry::{AGENT_TOOL_REGISTRY_SERVICE_ID, AgentToolRegistryServiceHandle},
    },
    app::{
        event::event_bus::{EVENT_BUS_SERVICE_ID, EventBusHandle},
        telemetry::{TELEMETRY_SERVICE_ID, TelemetryServiceHandle},
    },
    kosong::model::{MODEL_CATALOG_SERVICE_ID, ModelCatalogHandle},
    os::interface::{
        host_environment::{HOST_ENVIRONMENT_SERVICE_ID, HostEnvironmentHandle},
        host_file_system::{HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemServiceHandle},
    },
    session::{
        skill_catalog::{SESSION_SKILL_CATALOG_ID, SessionSkillCatalogHandle},
        workspace_context::{SESSION_WORKSPACE_CONTEXT_ID, SessionWorkspaceContextHandle},
    },
    tool::path_access::{WorkspaceConfig, extend_workspace_with_skill_roots},
};

#[derive(Default)]
struct RegistrationState {
    registration: Option<DisposableHandle>,
    registered_key: Option<String>,
}

pub struct AgentMediaToolsRegistrar {
    tool_registry: AgentToolRegistryServiceHandle,
    profile: AgentProfileServiceHandle,
    model_catalog: ModelCatalogHandle,
    fs: HostFileSystemServiceHandle,
    environment: HostEnvironmentHandle,
    workspace_context: SessionWorkspaceContextHandle,
    telemetry: TelemetryServiceHandle,
    skill_catalog: Option<SessionSkillCatalogHandle>,
    state: Mutex<RegistrationState>,
    disposables: DisposableStore,
}

impl AgentMediaToolsRegistrar {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tool_registry: AgentToolRegistryServiceHandle,
        profile: AgentProfileServiceHandle,
        model_catalog: ModelCatalogHandle,
        event_bus: EventBusHandle,
        fs: HostFileSystemServiceHandle,
        environment: HostEnvironmentHandle,
        workspace_context: SessionWorkspaceContextHandle,
        telemetry: TelemetryServiceHandle,
        skill_catalog: Option<SessionSkillCatalogHandle>,
    ) -> Result<Arc<Self>, String> {
        let service = Arc::new(Self {
            tool_registry,
            profile,
            model_catalog,
            fs,
            environment,
            workspace_context,
            telemetry,
            skill_catalog,
            state: Mutex::new(RegistrationState::default()),
            disposables: DisposableStore::new(),
        });
        service.refresh()?;
        let weak: Weak<Self> = Arc::downgrade(&service);
        service.disposables.add(event_bus.subscribe_type(
            "agent.status.updated",
            Arc::new(move |_| {
                if let Some(service) = weak.upgrade()
                    && let Err(error) = service.refresh()
                {
                    on_unexpected_error(&std::io::Error::other(error));
                }
            }),
        ));
        Ok(service)
    }

    fn refresh(&self) -> Result<(), String> {
        let capabilities = self
            .profile
            .get_model_capabilities()
            .map_err(|error| error.to_string())?;
        // TypeScript's getModel() returns an empty string before the first
        // binding. The Rust profile API represents that state as an error, so
        // consult has_model() to preserve the source registrar's bootstrap
        // behavior.
        let model_alias = if self.profile.has_model() {
            self.profile
                .get_model()
                .map_err(|error| error.to_string())?
        } else {
            String::new()
        };
        let key = format!(
            "{model_alias}|{}|{}",
            capabilities.image_in, capabilities.video_in
        );

        let mut state = self.state.lock();
        if state.registered_key.as_deref() == Some(&key) {
            return Ok(());
        }
        state.registered_key = Some(key);
        if let Some(registration) = state.registration.take() {
            registration.dispose().map_err(|error| error.to_string())?;
        }

        let workspace = WorkspaceConfig {
            workspace_dir: self
                .workspace_context
                .work_dir()
                .to_string_lossy()
                .into_owned(),
            additional_dirs: self
                .workspace_context
                .additional_dirs()
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        };
        let skill_roots = self
            .skill_catalog
            .as_ref()
            .map(|catalog| catalog.catalog().get_skill_roots())
            .unwrap_or_default();
        let path_class = self
            .environment
            .path_class()
            .map_err(|error| error.to_string())?;
        let workspace =
            extend_workspace_with_skill_roots(&workspace, &skill_roots, path_class).into_owned();

        let (requester, telemetry_props) = if model_alias.is_empty() {
            (None, VideoUploadTelemetryProps::default())
        } else {
            let requester = self
                .model_catalog
                .get_requester(&model_alias)
                .map_err(|error| error.to_string())?;
            let model = requester.model();
            (
                Some(requester),
                VideoUploadTelemetryProps {
                    model: Some(model_alias),
                    provider_type: Some(
                        model
                            .provider_type
                            .as_ref()
                            .map_or_else(|| model.protocol.to_string(), ToString::to_string),
                    ),
                    protocol: Some(model.protocol.to_string()),
                },
            )
        };
        let video_uploader = create_video_uploader(
            requester,
            Some(VideoUploadTelemetry {
                client: self.telemetry.clone(),
                props: telemetry_props,
            }),
        );
        state.registration = Some(register_media_tools(
            &self.tool_registry,
            RegisterMediaToolsDeps {
                fs: self.fs.clone(),
                environment: self.environment.clone(),
                workspace,
                capabilities,
                video_uploader,
                telemetry: Some(self.telemetry.clone()),
            },
        ));
        Ok(())
    }
}

impl AgentMediaToolsRegistrarContract for AgentMediaToolsRegistrar {}

impl Disposable for AgentMediaToolsRegistrar {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()?;
        if let Some(registration) = self.state.lock().registration.take() {
            registration.dispose()?;
        }
        Ok(())
    }
}

impl Drop for AgentMediaToolsRegistrar {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

pub fn register_agent_media_tools_registrar() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_MEDIA_TOOLS_REGISTRAR_ID,
        SyncDescriptor::new(|accessor| {
            let skill_catalog = accessor
                .get(SESSION_SKILL_CATALOG_ID)
                .ok()
                .map(|catalog| (*catalog).clone());
            let service = AgentMediaToolsRegistrar::new(
                (*accessor.get(AGENT_TOOL_REGISTRY_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_PROFILE_SERVICE_ID)?).clone(),
                (*accessor.get(MODEL_CATALOG_SERVICE_ID)?).clone(),
                (*accessor.get(EVENT_BUS_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_ENVIRONMENT_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?).clone(),
                (*accessor.get(TELEMETRY_SERVICE_ID)?).clone(),
                skill_catalog,
            )
            .map_err(DiError::Factory)?;
            let service: Arc<dyn AgentMediaToolsRegistrarContract> = service;
            Ok(AgentMediaToolsRegistrarHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "media",
    );
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use async_trait::async_trait;
    use futures_util::{StreamExt, stream};
    use serde_json::Map;

    use crate::{
        _base::{
            di::lifecycle::Disposable,
            errors::errors::BugIndicatingError,
            exec_env::environment_probe::{
                HostEnvironmentInfo, HostEnvironmentProbeError, PathClass, ShellName,
            },
        },
        agent::{
            profile::*,
            tool_registry::{AgentToolRegistryService, AgentToolRegistryServiceContract},
        },
        app::{
            event::{
                event_bus::{DomainEvent, EventBusContract},
                event_bus_service::EventBusService,
            },
            telemetry::{NoopTelemetryService, TelemetryServiceContract},
        },
        kosong::{
            contract::{
                capability::ModelCapability,
                provider::{ThinkingEffort, VideoUploadSource},
            },
            model::*,
            protocol::identity::Protocol,
        },
        os::{
            backends::node_local::host_fs_service::HostFileSystem,
            interface::{
                host_environment::{HostEnvironment, HostEnvironmentHandle},
                host_file_system::{HostFileSystemService, HostFileSystemServiceHandle},
            },
        },
        session::workspace_context::{
            PathAccessError, PathAccessOperation, SessionWorkspaceContextContract,
            SessionWorkspaceContextHandle,
        },
    };

    use super::*;

    struct Profile {
        state: Mutex<(String, ModelCapability)>,
    }

    impl Profile {
        fn bind_media(&self, alias: &str, capabilities: ModelCapability) {
            *self.state.lock() = (alias.into(), capabilities);
        }
    }

    #[async_trait]
    impl AgentProfileServiceContract for Profile {
        fn configure(&self, _: ProfileServiceOptions) {}
        fn update(&self, _: ProfileUpdateData) -> Result<(), ProfileServiceError> {
            unreachable!()
        }
        fn apply_binding_snapshot(
            &self,
            _: ProfileBindingSnapshot,
        ) -> Result<(), ProfileServiceError> {
            unreachable!()
        }
        async fn bind(&self, _: BindAgentInput) -> Result<(), ProfileServiceError> {
            unreachable!()
        }
        async fn set_model(&self, _: String) -> Result<ProfileSetModelResult, ProfileServiceError> {
            unreachable!()
        }
        fn set_thinking(&self, _: String) -> Result<(), ProfileServiceError> {
            unreachable!()
        }
        fn get_model(&self) -> Result<String, ProfileServiceError> {
            let alias = self.state.lock().0.clone();
            if alias.is_empty() {
                Err(Box::new(crate::_base::errors::errors::Error2::new(
                    "model.not_configured",
                    "Model not set",
                )))
            } else {
                Ok(alias)
            }
        }
        fn use_profile(
            &self,
            _: ResolvedAgentProfile,
            _: SystemPromptContext,
        ) -> Result<(), ProfileServiceError> {
            unreachable!()
        }
        async fn apply_profile(
            &self,
            _: ResolvedAgentProfile,
            _: Option<ApplyProfileOptions>,
        ) -> Result<(), ProfileServiceError> {
            unreachable!()
        }
        async fn refresh_system_prompt(&self) {}
        fn get_agents_md_warning(&self) -> Option<String> {
            None
        }
        fn data(&self) -> Result<ProfileData, ProfileServiceError> {
            unreachable!()
        }
        fn get_effective_thinking_level(&self) -> Result<ThinkingEffort, ProfileServiceError> {
            unreachable!()
        }
        fn resolve_model_context(&self) -> Result<ProfileModelContext, ProfileServiceError> {
            unreachable!()
        }
        fn resolve_request_params(&self) -> Result<ModelRequestParams, ProfileServiceError> {
            unreachable!()
        }
        fn get_model_capabilities(&self) -> Result<ModelCapability, ProfileServiceError> {
            Ok(self.state.lock().1.clone())
        }
        fn get_max_output_size(&self) -> Result<Option<u64>, ProfileServiceError> {
            unreachable!()
        }
        fn has_model(&self) -> bool {
            !self.state.lock().0.is_empty()
        }
        fn is_runnable(&self) -> bool {
            false
        }
        fn has_provider(&self) -> bool {
            false
        }
        fn get_system_prompt(&self) -> String {
            String::new()
        }
        fn get_active_tool_names(&self) -> Option<Vec<String>> {
            None
        }
        fn add_active_tool(&self, _: String) {}
        fn remove_active_tool(&self, _: &str) {}
    }

    struct Requester {
        model: Arc<Model>,
    }

    impl ModelRequester for Requester {
        fn model(&self) -> Arc<Model> {
            Arc::clone(&self.model)
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
        ) -> UploadVideoFuture {
            Box::pin(async { Ok(None) })
        }
    }

    struct Catalog;

    #[async_trait]
    impl ModelCatalogContract for Catalog {
        fn get(&self, id: &str) -> ModelCatalogResult<Arc<Model>> {
            Ok(test_model(id))
        }
        fn get_requester(&self, id: &str) -> ModelCatalogResult<Arc<dyn ModelRequester>> {
            Ok(Arc::new(Requester {
                model: test_model(id),
            }))
        }
        fn inspect(&self, _: &str) -> ModelCatalogResult<ModelInspection> {
            unreachable!()
        }
        async fn ping(&self, _: &str) -> ModelPingResult {
            unreachable!()
        }
        fn find_by_name(&self, _: &str) -> Vec<String> {
            Vec::new()
        }
        async fn list_models(&self) -> Vec<ModelCatalogItem> {
            Vec::new()
        }
        async fn list_providers(&self) -> ModelCatalogResult<Vec<ProviderCatalogItem>> {
            Ok(Vec::new())
        }
        async fn get_provider(&self, _: &str) -> ModelCatalogResult<ProviderCatalogItem> {
            unreachable!()
        }
        async fn set_default_model(&self, _: &str) -> ModelCatalogResult<SetDefaultModelResponse> {
            unreachable!()
        }
    }

    impl Disposable for Catalog {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    struct Environment;

    #[async_trait]
    impl HostEnvironment for Environment {
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

    struct Workspace;

    impl SessionWorkspaceContextContract for Workspace {
        fn work_dir(&self) -> PathBuf {
            PathBuf::from("/workspace")
        }
        fn additional_dirs(&self) -> Vec<PathBuf> {
            Vec::new()
        }
        fn set_work_dir(&self, _: &str) -> std::io::Result<()> {
            unreachable!()
        }
        fn set_additional_dirs(&self, _: &[String]) -> std::io::Result<()> {
            unreachable!()
        }
        fn resolve(&self, relative: &str) -> PathBuf {
            PathBuf::from("/workspace").join(relative)
        }
        fn is_within(&self, _: &str) -> bool {
            true
        }
        fn assert_allowed(
            &self,
            absolute_path: &str,
            _: PathAccessOperation,
        ) -> Result<PathBuf, PathAccessError> {
            Ok(PathBuf::from(absolute_path))
        }
        fn add_additional_dir(&self, _: &str) -> std::io::Result<()> {
            unreachable!()
        }
        fn remove_additional_dir(&self, _: &str) -> std::io::Result<()> {
            unreachable!()
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

    fn test_model(id: &str) -> Arc<Model> {
        Arc::new(Model {
            id: id.into(),
            name: id.into(),
            aliases: Vec::new(),
            protocol: Protocol::OpenAi,
            base_url: None,
            headers: indexmap::IndexMap::new(),
            capabilities: capabilities(true, true),
            max_context_size: 128_000,
            max_output_size: None,
            display_name: None,
            reasoning_key: None,
            reasoning_history: None,
            support_efforts: None,
            default_effort: None,
            always_thinking: false,
            provider_type: None,
            provider_name: "test".into(),
            auth_provider: Arc::new(StaticAuthProvider::new(None)),
            provider_options: None,
        })
    }

    struct Harness {
        registry: AgentToolRegistryServiceHandle,
        profile: Arc<Profile>,
        bus: Arc<EventBusService>,
        registrar: Arc<AgentMediaToolsRegistrar>,
    }

    fn harness() -> Harness {
        let registry: Arc<dyn AgentToolRegistryServiceContract> =
            Arc::new(AgentToolRegistryService::new());
        let registry = AgentToolRegistryServiceHandle(registry);
        let profile = Arc::new(Profile {
            state: Mutex::new((String::new(), capabilities(false, false))),
        });
        let profile_contract: Arc<dyn AgentProfileServiceContract> = profile.clone();
        let catalog: Arc<dyn ModelCatalogContract> = Arc::new(Catalog);
        let bus = Arc::new(EventBusService::new());
        let event_bus: Arc<dyn EventBusContract> = bus.clone();
        let fs: Arc<dyn HostFileSystemService> = Arc::new(HostFileSystem);
        let environment: Arc<dyn HostEnvironment> = Arc::new(Environment);
        let workspace: Arc<dyn SessionWorkspaceContextContract> = Arc::new(Workspace);
        let telemetry: Arc<dyn TelemetryServiceContract> = Arc::new(NoopTelemetryService);
        let registrar = AgentMediaToolsRegistrar::new(
            registry.clone(),
            AgentProfileServiceHandle(profile_contract),
            ModelCatalogHandle(catalog),
            EventBusHandle(event_bus),
            HostFileSystemServiceHandle(fs),
            HostEnvironmentHandle(environment),
            SessionWorkspaceContextHandle(workspace),
            TelemetryServiceHandle(telemetry),
            None,
        )
        .unwrap();
        Harness {
            registry,
            profile,
            bus,
            registrar,
        }
    }

    fn publish_status(harness: &Harness, alias: &str, capability: ModelCapability) {
        harness.profile.bind_media(alias, capability);
        harness
            .bus
            .publish(DomainEvent::new("agent.status.updated", Map::new()));
    }

    #[test]
    fn follows_capabilities_alias_changes_and_unregistration() {
        let harness = harness();
        assert!(harness.registry.resolve("ReadMediaFile").is_none());

        publish_status(&harness, "vision-a", capabilities(true, false));
        let first = harness.registry.resolve("ReadMediaFile").unwrap();
        assert!(
            first
                .tool()
                .description
                .contains("Video files are not supported")
        );

        publish_status(&harness, "vision-a", capabilities(true, false));
        let unchanged = harness.registry.resolve("ReadMediaFile").unwrap();
        assert!(Arc::ptr_eq(&first, &unchanged));

        publish_status(&harness, "vision-b", capabilities(true, false));
        let second = harness.registry.resolve("ReadMediaFile").unwrap();
        assert!(!Arc::ptr_eq(&first, &second));

        publish_status(&harness, "text", capabilities(false, false));
        assert!(harness.registry.resolve("ReadMediaFile").is_none());

        publish_status(&harness, "vision-c", capabilities(true, true));
        assert!(harness.registry.resolve("ReadMediaFile").is_some());
        harness.registrar.dispose().unwrap();
        assert!(harness.registry.resolve("ReadMediaFile").is_none());
    }

    #[test]
    fn registration_is_eager_agent_scoped_in_media_domain() {
        register_agent_media_tools_registrar();
        let descriptor =
            crate::_base::di::scope::get_scoped_service_descriptors(LifecycleScope::Agent)
                .into_iter()
                .find(|entry| entry.id.to_string() == AGENT_MEDIA_TOOLS_REGISTRAR_ID.to_string())
                .expect("media tools registrar is registered");
        assert!(!descriptor.descriptor.supports_delayed_instantiation);
        assert_eq!(descriptor.domain, "media");
    }
}
