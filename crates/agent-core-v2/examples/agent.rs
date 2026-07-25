//! Minimal interactive agent using the built-in request pipeline.
//!
//! The assembly follows `packages/agent-core-v2/test/harness/agent.ts` while
//! the Rust application/session lifecycle composition roots are still being
//! migrated. LLM transport, OAuth, model resolution, projection, usage
//! accounting, and turn execution all use library services.

use std::{
    collections::HashMap,
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, Once},
};

use kimi_code_agent_core_v2::{
    _base::{
        di::{
            instantiation_service::InstantiationService, lifecycle::Disposable,
            service_collection::ServiceCollection,
        },
        log::{AppLogService, LogService, LogServiceHandle, Logger, resolve_logging_config},
    },
    agent::{
        blob::AgentBlobService,
        context_injector::{AgentContextInjectorService, AgentContextInjectorServiceContract},
        context_memory::{
            AgentContextMemoryService, AgentContextMemoryServiceContract,
            AgentContextMemoryServiceHandle, ContextMessage, PromptOrigin,
        },
        context_projector::{
            AgentContextProjectorService, AgentContextProjectorServiceContract,
            AgentContextProjectorServiceHandle,
        },
        context_size::{
            AgentContextSizeService, AgentContextSizeServiceContract, AgentContextSizeServiceHandle,
        },
        fault_injection::{
            FaultInjectionService, FaultInjectionServiceContract, FaultInjectionServiceHandle,
        },
        llm_requester::{AgentLlmRequesterService, AgentLlmRequesterServiceContract},
        loop_::{
            AgentLoopService, AgentLoopServiceContract, AgentLoopServiceHandle, LoopRunResult,
            StepRequest, register_loop_control_config_section,
        },
        profile::{
            AgentProfileService, AgentProfileServiceContract, AgentProfileServiceHandle,
            BindAgentInput,
        },
        prompt::PromptStepRequest,
        scope_context::{AgentScopeContextInput, make_agent_scope_context},
        system_reminder::{AgentSystemReminderService, AgentSystemReminderServiceContract},
        task::{
            AGENT_TASK_SERVICE_ID, AgentTaskService, AgentTaskServiceContract,
            AgentTaskServiceHandle, register_task_config_sections,
            tools::{register_task_list_tool, register_task_output_tool, register_task_stop_tool},
        },
        tool_executor::{
            AgentToolExecutorService, AgentToolExecutorServiceContract,
            AgentToolExecutorServiceHandle,
        },
        tool_policy::{
            AgentToolPolicyService, AgentToolPolicyServiceContract, AgentToolPolicyServiceHandle,
            register_tools_config_section,
        },
        tool_registry::{
            AgentBuiltinToolsRegistrar, AgentToolRegistryService, AgentToolRegistryServiceContract,
            AgentToolRegistryServiceHandle,
        },
        tool_result_truncation::{
            AgentToolResultTruncationServiceContract, ToolResultTruncationService,
        },
        tool_select::{
            AGENT_TOOL_SELECT_SERVICE_ID, AgentToolSelectService, AgentToolSelectServiceContract,
            AgentToolSelectServiceHandle, register_select_tools_tool,
        },
        usage::{AgentUsageService, AgentUsageServiceContract, AgentUsageServiceHandle},
    },
    app::{
        agent_file_catalog::{
            AgentCatalogRuntimeOptions, UserFileAgentSource, UserFileAgentSourceHandle,
            register_agent_file_catalog_config_sections,
        },
        agent_profile_catalog::{
            AgentProfileCatalogContract, AgentProfileCatalogHandle, AgentProfileCatalogService,
        },
        auth::{
            OAuthService, OAuthServiceContract, OAuthServiceHandle, OAuthToolkitContract,
            OAuthToolkitHandle, OAuthToolkitService, register_services_config_section,
        },
        bootstrap::{
            BootstrapInput, BootstrapService, BootstrapServiceContract, BootstrapServiceHandle,
            resolve_bootstrap_options,
        },
        config::{
            ConfigRegistry, ConfigRegistryContract, ConfigRegistryHandle, ConfigService,
            ConfigServiceContract, ConfigServiceHandle, ConfigTarget,
        },
        event::{
            EventService, EventServiceContract, EventServiceHandle,
            event_bus::{DomainEvent, EventBusContract, EventBusHandle},
            event_bus_service::EventBusService,
        },
        flag::{
            FlagRegistry, FlagRegistryHandle, FlagRegistryService, FlagService,
            FlagServiceContract, FlagServiceHandle, register_experimental_config_section,
        },
        plugin::{
            PluginService, PluginServiceContract, PluginServiceHandle,
            ensure_plugin_errors_registered,
        },
        skill_catalog::{
            BuiltinSkillSource, FileSkillDiscovery, SkillCatalogRuntimeOptions,
            SkillDiscoveryHandle, SkillSourceContract, UserFileSkillSource,
            ensure_skill_errors_registered, register_skill_catalog_config_sections,
        },
        telemetry::{
            AgentTelemetryContextService, AgentTelemetryContextServiceContract,
            AgentTelemetryContextServiceHandle, TelemetryService, TelemetryServiceContract,
            TelemetryServiceHandle,
        },
        web::{
            WEB_FETCH_SERVICE_ID, WebFetchService, WebFetchServiceContract, WebFetchServiceHandle,
            register_fetch_url_tool,
        },
        workspace_registry::{
            FileWorkspacePersistence, WorkspacePersistenceContract, WorkspaceRegistryContract,
            WorkspaceRegistryService,
        },
    },
    kosong::{
        contract::message::{ContentPart, Message, Role},
        model::{
            HostRequestHeaders, ModelCatalog, ModelCatalogContract, ModelCatalogHandle,
            ModelService,
            contract::{ModelServiceContract, ModelServiceHandle},
            register_models_config_section,
            thinking::register_thinking_config_section,
        },
        protocol::identity::{ProtocolAdapterRegistry, ProtocolAdapterRegistryHandle},
        provider::{
            ProviderService, ProviderServiceContract, ProviderServiceHandle,
            bases::{
                anthropic::anthropic_contrib::ensure_anthropic_base_registered,
                google_genai::google_genai_contrib::ensure_google_gen_ai_base_registered,
                openai::{
                    openai_legacy_contrib::ensure_openai_legacy_base_registered,
                    openai_responses_contrib::ensure_openai_responses_base_registered,
                },
            },
            protocol_adapter_registry::ProtocolAdapterRegistry as ConcreteProtocolAdapterRegistry,
            providers::ensure_provider_definitions_registered,
            register_provider_config_section,
        },
    },
    os::{
        backends::node_local::{
            host_environment_service::LocalHostEnvironmentService, host_fs_service::HostFileSystem,
            host_process_service::LocalHostProcessService, tools::register_bash_tool,
        },
        interface::{
            host_environment::{HostEnvironment, HostEnvironmentHandle},
            host_file_system::{HostFileSystemService, HostFileSystemServiceHandle},
            host_process::{HostProcessService, HostProcessServiceHandle},
        },
    },
    persistence::{
        backends::node_fs::{
            append_log_store::AppendLogStore,
            atomic_document_store::{JsonAtomicDocumentStore, TomlAtomicDocumentStore},
            blob_store_service::BlobStoreService,
            file_storage_service::FileStorageService,
        },
        interface::{
            append_log_store::AppendLogStoreHandle,
            atomic_document_store::{AtomicDocumentStoreHandle, AtomicDocumentStoreService},
            blob_store::BlobStoreHandle,
            storage::{FileSystemStorageService, FileSystemStorageServiceHandle},
        },
    },
    session::{
        agent_profile_catalog::{
            ExplicitFileAgentSource, ExtraFileAgentSource, ProjectFileAgentSource,
            SessionAgentProfileCatalogContract, SessionAgentProfileCatalogHandle,
            SessionAgentProfileCatalogService,
        },
        process::{
            SESSION_PROCESS_RUNNER_SERVICE_ID, SessionProcessRunner, SessionProcessRunnerContract,
            SessionProcessRunnerHandle,
        },
        session_context::{SESSION_CONTEXT_ID, SessionContextInput, make_session_context},
        session_metadata::{AgentMeta, AgentMetaType, SessionMetadataService},
        skill_catalog::{
            ExplicitFileSkillSource, ExtraFileSkillSource, PluginSkillSource,
            SessionSkillCatalogContract, SessionSkillCatalogHandle, SessionSkillCatalogService,
            WorkspaceFileSkillSource,
        },
        tool_policy::{
            SessionToolPolicyContract, SessionToolPolicyHandle, SessionToolPolicyService,
        },
        workspace_context::{
            SessionWorkspaceContextContract, SessionWorkspaceContextHandle,
            SessionWorkspaceContextService,
        },
    },
    wire::{
        contract::WireServiceHandle,
        wire_service::{DomainEventPublisher, WireService},
    },
};
use kimi_code_oauth::{
    CredentialKind, KIMI_CODE_PROVIDER_NAME, ManagedKimiCodeApplyOptions,
    apply_managed_kimi_code_config, fetch_managed_kimi_code_models,
};
use serde_json::{Map, Value};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (prompt, requested_cwd) = read_arguments()?;
    let bootstrap_options = resolve_bootstrap_options(BootstrapInput {
        cwd: requested_cwd,
        ..BootstrapInput::default()
    })?;
    let cwd = bootstrap_options.cwd.to_string_lossy().into_owned();

    let toolkit = Arc::new(OAuthToolkitService::new(&bootstrap_options.home_dir)?);
    let access_token = toolkit
        .get_cached_access_token(Some(KIMI_CODE_PROVIDER_NAME), None)
        .await?
        .ok_or("尚未登录，请先运行：cargo run -p kimi-code-agent-core-v2 --example login")?;
    println!("access_token={access_token}");
    let managed_models =
        fetch_managed_kimi_code_models(&access_token, None, None, CredentialKind::OAuth).await?;
    if managed_models.is_empty() {
        return Err("当前账号没有可用模型".into());
    }
    println!("managed_models={managed_models:?}");

    let mut initial_config = Map::new();
    apply_managed_kimi_code_config(
        &mut initial_config,
        ManagedKimiCodeApplyOptions {
            models: &managed_models,
            base_url: None,
            oauth_key: None,
            oauth_host: None,
            preserve_default_model: false,
        },
    )?;
    let model_alias = initial_config
        .get("defaultModel")
        .and_then(Value::as_str)
        .ok_or("OAuth 模型目录没有默认模型")?
        .to_owned();

    ensure_protocols_registered()?;
    ensure_config_sections_registered();
    let bootstrap: Arc<dyn BootstrapServiceContract> =
        Arc::new(BootstrapService::new(bootstrap_options.clone()));
    let bootstrap_handle = BootstrapServiceHandle(bootstrap);
    let storage: Arc<dyn FileSystemStorageService> = Arc::new(
        FileStorageService::with_default_modes(bootstrap_options.home_dir.clone()),
    );
    let fs: Arc<dyn HostFileSystemService> = Arc::new(HostFileSystem);
    let fs_handle = HostFileSystemServiceHandle(Arc::clone(&fs));
    let json_documents: Arc<dyn AtomicDocumentStoreService> =
        Arc::new(JsonAtomicDocumentStore::new(Arc::clone(&storage)));
    let json_documents_handle = AtomicDocumentStoreHandle(Arc::clone(&json_documents));
    let toml_documents: Arc<dyn AtomicDocumentStoreService> =
        Arc::new(TomlAtomicDocumentStore::new(Arc::clone(&storage)));
    let toml_documents_handle = AtomicDocumentStoreHandle(toml_documents);
    let telemetry: Arc<dyn TelemetryServiceContract> = Arc::new(TelemetryService::new());
    let telemetry_handle = TelemetryServiceHandle(Arc::clone(&telemetry));
    let telemetry_context: Arc<dyn AgentTelemetryContextServiceContract> =
        Arc::new(AgentTelemetryContextService::new());
    let telemetry_context_handle =
        AgentTelemetryContextServiceHandle(Arc::clone(&telemetry_context));
    let logging = resolve_logging_config(&bootstrap_options.home_dir, &HashMap::new());
    let log: Arc<dyn LogService> = Arc::new(AppLogService::new(&logging));
    let log_handle = LogServiceHandle(log);
    let config_registry: Arc<dyn ConfigRegistryContract> = Arc::new(ConfigRegistry::new()?);
    let config: Arc<dyn ConfigServiceContract> = ConfigService::new(
        ConfigRegistryHandle(config_registry),
        bootstrap_handle.clone(),
        log_handle.clone(),
        toml_documents_handle,
    );
    config.ready().await?;
    for (domain, value) in initial_config {
        config
            .replace(&domain, Some(value), ConfigTarget::Memory)
            .await?;
    }
    let config_handle = ConfigServiceHandle(Arc::clone(&config));
    let workspace_persistence: Arc<dyn WorkspacePersistenceContract> =
        Arc::new(FileWorkspacePersistence::new(Arc::clone(&json_documents)));
    let workspace_registry =
        WorkspaceRegistryService::new(workspace_persistence, Arc::clone(&storage), Arc::clone(&fs));
    let workspace_record = workspace_registry.create_or_touch(&cwd, None).await?;
    let workspace_id = workspace_record.id;
    let session_id = "test".to_owned();
    let events: Arc<dyn EventServiceContract> = Arc::new(EventService::new());
    let event_handle = EventServiceHandle(events);

    let provider_service: Arc<dyn ProviderServiceContract> =
        Arc::new(ProviderService::new(config_handle.clone()));
    let provider_handle = ProviderServiceHandle(provider_service);
    let model_service: Arc<dyn ModelServiceContract> =
        Arc::new(ModelService::new(config_handle.clone()));
    let model_handle = ModelServiceHandle(model_service);
    let toolkit_handle = OAuthToolkitHandle(toolkit);
    let oauth: Arc<dyn OAuthServiceContract> = Arc::new(OAuthService::new(
        toolkit_handle,
        provider_handle.clone(),
        config_handle.clone(),
        telemetry_handle.clone(),
        log_handle.clone(),
        event_handle,
    ));
    let oauth_handle = OAuthServiceHandle(oauth);
    let protocol_registry: Arc<dyn ProtocolAdapterRegistry> =
        Arc::new(ConcreteProtocolAdapterRegistry::new());
    let protocol_handle = ProtocolAdapterRegistryHandle(Arc::clone(&protocol_registry));
    let host_headers = HostRequestHeaders::new(Default::default());
    let catalog: Arc<dyn ModelCatalogContract> = Arc::new(ModelCatalog::new(
        config_handle.clone(),
        provider_handle.clone(),
        model_handle,
        oauth_handle.clone(),
        protocol_handle.clone(),
        host_headers.clone(),
    ));
    let catalog_handle = ModelCatalogHandle(catalog);

    let session_scope = bootstrap_handle.session_scope(&workspace_id, &session_id);
    let session_dir = bootstrap_handle.session_dir(&workspace_id, &session_id);
    let agent_homedir = bootstrap_handle.agent_homedir(&workspace_id, &session_id, "main");
    let session_context = Arc::new(make_session_context(SessionContextInput {
        session_id: session_id.clone(),
        workspace_id: workspace_id.clone(),
        session_dir: session_dir.to_string_lossy().into_owned(),
        session_scope,
        cwd: cwd.clone(),
        meta_scope: None,
    }));
    let agent_scope = make_agent_scope_context(AgentScopeContextInput {
        agent_id: "main".into(),
        agent_scope: bootstrap_handle.agent_scope(&workspace_id, &session_id, "main"),
    });
    let session_metadata =
        SessionMetadataService::new(&session_context, json_documents_handle.clone());
    let blob_store = BlobStoreHandle(Arc::new(BlobStoreService::new(Arc::clone(&storage))));
    let agent_blobs = Arc::new(AgentBlobService::new(blob_store, &agent_scope));

    let event_bus = Arc::new(EventBusService::new());
    let event_bus_contract: Arc<dyn EventBusContract> = event_bus.clone();
    let event_bus_handle = EventBusHandle(Arc::clone(&event_bus_contract));
    let publisher: Arc<dyn DomainEventPublisher> = event_bus.clone();
    let wire = Arc::new(WireService::new(
        agent_scope.scope(None),
        AppendLogStoreHandle(Arc::new(AppendLogStore::new(Arc::clone(&storage)))),
        agent_blobs,
        publisher,
    ));
    let wire_handle = WireServiceHandle(Arc::clone(&wire));
    let context: Arc<dyn AgentContextMemoryServiceContract> = Arc::new(
        AgentContextMemoryService::new(Arc::clone(&wire), Arc::clone(&event_bus_contract)),
    );
    let context_handle = AgentContextMemoryServiceHandle(Arc::clone(&context));

    let workspace: Arc<dyn SessionWorkspaceContextContract> =
        Arc::new(SessionWorkspaceContextService::new(&session_context)?);
    let workspace_handle = SessionWorkspaceContextHandle(workspace);
    let logger: Arc<dyn Logger> = log_handle.0.clone();

    let builtin_catalog = Arc::new(AgentProfileCatalogService::new());
    let builtin: Arc<dyn AgentProfileCatalogContract> = builtin_catalog;
    let builtin_handle = AgentProfileCatalogHandle(Arc::clone(&builtin));

    let user_agent_source = Arc::new(UserFileAgentSource::new(
        bootstrap_handle.clone(),
        fs_handle.clone(),
        Arc::clone(&logger),
        builtin_handle.clone(),
    )?);
    let user_agent_source_handle = UserFileAgentSourceHandle(Arc::clone(&user_agent_source));
    let extra_agent_source = ExtraFileAgentSource::new(
        config_handle.clone(),
        workspace_handle.clone(),
        bootstrap_handle.clone(),
        fs_handle.clone(),
        Arc::clone(&logger),
        user_agent_source_handle.clone(),
    );
    let project_agent_source = Arc::new(ProjectFileAgentSource::new(
        workspace_handle.clone(),
        fs_handle.clone(),
        Arc::clone(&logger),
        user_agent_source_handle.clone(),
    ));
    let explicit_agent_source = Arc::new(ExplicitFileAgentSource::new(
        Arc::new(AgentCatalogRuntimeOptions::default()),
        workspace_handle.clone(),
        bootstrap_handle.clone(),
        fs_handle.clone(),
        user_agent_source_handle,
    ));
    let session_catalog: Arc<dyn SessionAgentProfileCatalogContract> =
        SessionAgentProfileCatalogService::new(
            builtin_handle.clone(),
            user_agent_source,
            extra_agent_source,
            project_agent_source,
            explicit_agent_source,
            Arc::clone(&logger),
        );
    session_catalog.ready().await?;
    let session_catalog_handle = SessionAgentProfileCatalogHandle(Arc::clone(&session_catalog));

    let skill_discovery =
        SkillDiscoveryHandle(Arc::new(FileSkillDiscovery::new(log_handle.clone())));
    let skill_runtime_options = Arc::new(SkillCatalogRuntimeOptions::default());
    ensure_plugin_errors_registered();
    ensure_skill_errors_registered();
    let plugin_service: Arc<dyn PluginServiceContract> = Arc::new(PluginService::new(
        bootstrap_handle.clone(),
        skill_discovery.clone(),
        provider_handle.clone(),
    ));
    let plugin_service_handle = PluginServiceHandle(Arc::clone(&plugin_service));
    let builtin_skill_source: Arc<dyn SkillSourceContract> = Arc::new(BuiltinSkillSource);
    let user_skill_source = UserFileSkillSource::new(
        skill_discovery.clone(),
        bootstrap_handle.clone(),
        config_handle.clone(),
        Arc::clone(&skill_runtime_options),
    );
    let explicit_skill_source = Arc::new(ExplicitFileSkillSource::new(
        skill_discovery.clone(),
        Arc::clone(&skill_runtime_options),
        workspace_handle.clone(),
        bootstrap_handle.clone(),
    ));
    let extra_skill_source = ExtraFileSkillSource::new(
        skill_discovery.clone(),
        config_handle.clone(),
        workspace_handle.clone(),
        bootstrap_handle.clone(),
    );
    let workspace_skill_source = WorkspaceFileSkillSource::new(
        skill_discovery.clone(),
        workspace_handle.clone(),
        config_handle.clone(),
        skill_runtime_options,
    );
    let plugin_skill_source = Arc::new(PluginSkillSource::new(
        skill_discovery,
        plugin_service_handle,
    ));
    let skills: Arc<dyn SessionSkillCatalogContract> = SessionSkillCatalogService::new(
        builtin_skill_source,
        user_skill_source,
        explicit_skill_source,
        extra_skill_source,
        workspace_skill_source,
        plugin_skill_source,
    );
    skills.ready().await?;
    let skills_handle = SessionSkillCatalogHandle(Arc::clone(&skills));
    let session_policy: Arc<dyn SessionToolPolicyContract> = Arc::new(
        SessionToolPolicyService::new(&session_context, json_documents_handle.clone()),
    );
    session_policy.ready().await?;
    let session_policy_handle = SessionToolPolicyHandle(session_policy);
    let host_env = Arc::new(LocalHostEnvironmentService::new());
    host_env.ready().await?;
    let host_env_contract: Arc<dyn HostEnvironment> = host_env;
    let host_env_handle = HostEnvironmentHandle(host_env_contract);
    let registry: Arc<dyn AgentToolRegistryServiceContract> =
        Arc::new(AgentToolRegistryService::new());
    let registry_handle = AgentToolRegistryServiceHandle(Arc::clone(&registry));

    let profile: Arc<dyn AgentProfileServiceContract> = Arc::new(AgentProfileService::new(
        wire_handle.clone(),
        event_bus_handle.clone(),
        telemetry_handle.clone(),
        telemetry_context_handle.clone(),
        config_handle.clone(),
        catalog_handle.clone(),
        protocol_handle,
        host_env_handle.clone(),
        fs_handle,
        Arc::clone(&session_context),
        bootstrap_handle.clone(),
        workspace_handle,
        session_catalog_handle,
        skills_handle,
        session_policy_handle.clone(),
        registry_handle.clone(),
        builtin_handle,
    ));
    let profile_handle = AgentProfileServiceHandle(Arc::clone(&profile));

    let truncation: Arc<dyn AgentToolResultTruncationServiceContract> =
        Arc::new(ToolResultTruncationService::new(
            bootstrap_handle.clone(),
            &agent_scope,
            FileSystemStorageServiceHandle(Arc::clone(&storage)),
        ));
    let tools: Arc<dyn AgentToolExecutorServiceContract> = Arc::new(AgentToolExecutorService::new(
        Arc::clone(&registry),
        event_bus_handle.clone(),
        telemetry_handle.clone(),
        truncation,
    ));
    let tools_handle = AgentToolExecutorServiceHandle(Arc::clone(&tools));
    let policy: Arc<dyn AgentToolPolicyServiceContract> = Arc::new(AgentToolPolicyService::new(
        profile_handle.clone(),
        config_handle.clone(),
        session_policy_handle.clone(),
        tools_handle.clone(),
    ));
    let policy_handle = AgentToolPolicyServiceHandle(policy);
    let flag_registry = Arc::new(FlagRegistryService::new()?);
    let flag_registry_contract: Arc<dyn FlagRegistry> = flag_registry;
    let flags: Arc<dyn FlagServiceContract> = FlagService::new(
        bootstrap_handle,
        config_handle.clone(),
        FlagRegistryHandle(flag_registry_contract),
    );
    let flags_handle = FlagServiceHandle(flags);
    let tool_select: Arc<dyn AgentToolSelectServiceContract> =
        Arc::new(AgentToolSelectService::new(
            Arc::clone(&registry),
            profile_handle.clone(),
            policy_handle.clone(),
            Arc::clone(&context),
            tools_handle,
            flags_handle.clone(),
            event_bus_handle.clone(),
        ));
    let tool_select_handle = AgentToolSelectServiceHandle(tool_select);
    let web_fetch: Arc<dyn WebFetchServiceContract> = Arc::new(WebFetchService::new(
        provider_handle,
        oauth_handle,
        host_headers,
    ));
    let web_fetch_handle = WebFetchServiceHandle(web_fetch);

    let projector: Arc<dyn AgentContextProjectorServiceContract> = Arc::new(
        AgentContextProjectorService::new(log_handle, telemetry_handle.clone()),
    );
    let projector_handle = AgentContextProjectorServiceHandle(projector);
    let context_size: Arc<dyn AgentContextSizeServiceContract> = Arc::new(
        AgentContextSizeService::new(Arc::clone(&context), Arc::clone(&wire)),
    );
    let context_size_handle = AgentContextSizeServiceHandle(context_size);
    let usage: Arc<dyn AgentUsageServiceContract> = Arc::new(AgentUsageService::new(
        Arc::clone(&wire),
        Some(Arc::clone(&event_bus_contract)),
    ));
    let usage_handle = AgentUsageServiceHandle(usage);
    let fault: Arc<dyn FaultInjectionServiceContract> =
        Arc::new(FaultInjectionService::from_flag_service(flags_handle));
    let fault_handle = FaultInjectionServiceHandle(fault);
    let requester: Arc<dyn AgentLlmRequesterServiceContract> =
        Arc::new(AgentLlmRequesterService::new(
            context_handle,
            projector_handle,
            context_size_handle,
            registry_handle.clone(),
            tool_select_handle.clone(),
            profile_handle,
            usage_handle,
            catalog_handle,
            fault_handle,
        ));
    let agent = AgentLoopService::new(
        Arc::clone(&context),
        requester,
        Arc::clone(&event_bus_contract),
        tools,
        Arc::clone(&config),
        wire_handle.clone(),
        telemetry,
        telemetry_context,
    );
    let agent_contract: Arc<dyn AgentLoopServiceContract> = agent.clone();
    let reminders: Arc<dyn AgentSystemReminderServiceContract> =
        Arc::new(AgentSystemReminderService::new(Arc::clone(&context)));
    let injector = AgentContextInjectorService::new(
        Arc::clone(&context),
        AgentLoopServiceHandle(Arc::clone(&agent_contract)),
        Arc::clone(&reminders),
        event_bus_handle.clone(),
        wire_handle.clone(),
    )?;
    let injector_contract: Arc<dyn AgentContextInjectorServiceContract> = injector.clone();
    let task_service: Arc<dyn AgentTaskServiceContract> =
        Arc::new(AgentTaskService::from_dependencies(
            telemetry_handle,
            Arc::clone(&context),
            config_handle.clone(),
            json_documents_handle,
            FileSystemStorageServiceHandle(Arc::clone(&storage)),
            &session_context,
            &agent_scope,
            wire_handle,
            event_bus_handle,
            injector_contract.as_ref(),
            agent_contract,
        )?);
    let task_handle = AgentTaskServiceHandle(Arc::clone(&task_service));
    let host_process: Arc<dyn HostProcessService> = Arc::new(LocalHostProcessService::default());
    let process_runner: Arc<dyn SessionProcessRunnerContract> =
        Arc::new(SessionProcessRunner::new(
            Arc::clone(&session_context),
            HostProcessServiceHandle(host_process),
        ));
    let process_runner_handle = SessionProcessRunnerHandle(process_runner);

    ensure_builtin_tool_contributions_registered();
    let mut tool_services = ServiceCollection::new();
    tool_services.set_instance(
        AGENT_TOOL_SELECT_SERVICE_ID,
        Arc::new(tool_select_handle.clone()),
    );
    tool_services.set_instance(WEB_FETCH_SERVICE_ID, Arc::new(web_fetch_handle));
    tool_services.set_instance(SESSION_CONTEXT_ID, Arc::clone(&session_context));
    tool_services.set_instance(
        SESSION_PROCESS_RUNNER_SERVICE_ID,
        Arc::new(process_runner_handle),
    );
    tool_services.set_instance(
        kimi_code_agent_core_v2::os::interface::host_environment::HOST_ENVIRONMENT_SERVICE_ID,
        Arc::new(host_env_handle),
    );
    tool_services.set_instance(AGENT_TASK_SERVICE_ID, Arc::new(task_handle));
    tool_services.set_instance(
        kimi_code_agent_core_v2::agent::tool_policy::AGENT_TOOL_POLICY_SERVICE_ID,
        Arc::new(policy_handle),
    );
    tool_services.set_instance(
        kimi_code_agent_core_v2::app::config::CONFIG_SERVICE_ID,
        Arc::new(config_handle.clone()),
    );
    let tool_services = InstantiationService::new(tool_services);

    wire.seal().await?;
    session_metadata
        .register_agent(
            "main".into(),
            AgentMeta {
                homedir: Some(agent_homedir.to_string_lossy().into_owned()),
                r#type: Some(AgentMetaType::Main),
                ..AgentMeta::default()
            },
        )
        .await?;
    wire.restore().await?;
    eprintln!("Session {}: {}", session_id, session_dir.to_string_lossy());
    profile
        .bind(BindAgentInput {
            profile: "agent".into(),
            model: Some(model_alias),
            thinking: None,
            strict_thinking: None,
            cwd: Some(cwd),
        })
        .await?;
    let builtin_tools = AgentBuiltinToolsRegistrar::new(&tool_services, &registry_handle)?;

    let _assistant_output = event_bus.subscribe_type(
        "assistant.delta",
        Arc::new(|event: &DomainEvent| {
            if let Some(text) = event.fields.get("delta").and_then(Value::as_str) {
                print!("{text}");
                let _ = io::stdout().flush();
            }
        }),
    );

    let request: Arc<dyn StepRequest> = Arc::new(PromptStepRequest::new(
        ContextMessage {
            message: Message::new(
                Role::User,
                vec![ContentPart::Text { text: prompt }],
                Vec::new(),
            ),
            id: Some(uuid::Uuid::new_v4().to_string()),
            provider_message_id: None,
            origin: Some(PromptOrigin::User),
            is_error: None,
            note: None,
        },
        Vec::new(),
        reminders,
    ));
    let assignment = agent.enqueue(request, None)?.assigned.await?;
    let result = assignment.turn.result().await;
    println!();

    match result {
        LoopRunResult::Completed {
            steps, truncated, ..
        } => eprintln!("Agent 完成：steps={steps}, truncated={truncated}"),
        LoopRunResult::Failed { error, .. } => return Err(error.into()),
        LoopRunResult::Cancelled { reason, .. } => return Err(reason.into()),
    }

    wire.flush().await?;
    Disposable::dispose(agent.as_ref())?;
    Disposable::dispose(&builtin_tools)?;
    Disposable::dispose(&tool_services)?;
    Disposable::dispose(task_service.as_ref())?;
    Disposable::dispose(injector.as_ref())?;
    Disposable::dispose(session_catalog.as_ref())?;
    Disposable::dispose(skills.as_ref())?;
    Disposable::dispose(plugin_service.as_ref())?;
    Disposable::dispose(config.as_ref())?;
    Ok(())
}

fn read_arguments() -> Result<(String, Option<PathBuf>), io::Error> {
    let mut cwd = None;
    let mut prompt = Vec::new();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--cwd" {
            cwd = arguments.next().map(PathBuf::from);
        } else {
            prompt.push(argument);
        }
    }
    if !prompt.is_empty() {
        return Ok((prompt.join(" "), cwd));
    }
    print!("请输入提示词：");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok((input.trim().to_owned(), cwd))
}

fn ensure_protocols_registered() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ensure_provider_definitions_registered()?;
    ensure_openai_legacy_base_registered()?;
    ensure_openai_responses_base_registered()?;
    ensure_anthropic_base_registered()?;
    ensure_google_gen_ai_base_registered()?;
    Ok(())
}

fn ensure_config_sections_registered() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        register_provider_config_section();
        register_models_config_section();
        register_thinking_config_section();
        register_services_config_section();
        register_experimental_config_section();
        register_tools_config_section();
        register_loop_control_config_section();
        register_agent_file_catalog_config_sections();
        register_skill_catalog_config_sections();
        register_task_config_sections();
    });
}

fn ensure_builtin_tool_contributions_registered() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        register_select_tools_tool();
        register_fetch_url_tool();
        register_task_list_tool();
        register_task_output_tool();
        register_task_stop_tool();
        register_bash_tool();
    });
}
