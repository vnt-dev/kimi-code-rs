//! Agent-scoped profile service.
//!
//! This is the Rust counterpart of
//! `packages/agent-core-v2/src/agent/profile/profileService.ts`.

use std::{
    collections::HashSet,
    path::PathBuf,
};
use std::sync::{Arc};
use parking_lot::Mutex;

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::ServicesAccessorExt,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    agent::{
        loop_::{LOOP_CONTROL_SECTION, LoopControl},
        tool_policy::{
            GlobalToolsPolicy, InactiveToolPattern, InactiveToolPatternKind, TOOLS_SECTION,
            ToolActivationPolicy, ToolPolicyLayers, ToolsConfig, find_inactive_tool_patterns,
            is_tool_active_composed, literal_tool_names,
        },
        tool_registry::{AGENT_TOOL_REGISTRY_SERVICE_ID, AgentToolRegistryServiceHandle},
    },
    app::{
        agent_profile_catalog::{
            AGENT_PROFILE_CATALOG_SERVICE_ID, AgentProfile, AgentProfileCatalogHandle,
            AgentProfileContext, DEFAULT_AGENT_PROFILE_NAME,
        },
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
        event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID, EventBusHandle},
        telemetry::{
            AGENT_TELEMETRY_CONTEXT_SERVICE_ID, AgentTelemetryContextPatch,
            AgentTelemetryContextServiceHandle, TELEMETRY_SERVICE_ID, TelemetryServiceHandle,
        },
    },
    kosong::{
        contract::{
            capability::UNKNOWN_CAPABILITY,
            provider::{SamplingOptions, ThinkingEffort},
        },
        model::{
            MODEL_CATALOG_SERVICE_ID, Model, ModelCatalogHandle, ModelRequestParams,
            thinking::{
                THINKING_SECTION, ThinkingConfig, drives_thinking_through_traits,
                model_supports_thinking_effort, normalize_requested_thinking_effort,
                requires_strict_thinking_validation, resolve_forced_thinking_effort,
                resolve_thinking_effort_for_model, resolve_thinking_keep,
            },
            types::ModelOverrides,
            types::{ModelThinkingCapabilities, ModelThinkingMetadata},
        },
        protocol::identity::{PROTOCOL_ADAPTER_REGISTRY_SERVICE_ID, ProtocolAdapterRegistryHandle},
    },
    os::interface::{
        host_environment::{HOST_ENVIRONMENT_SERVICE_ID, HostEnvironmentHandle},
        host_file_system::{HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemServiceHandle},
    },
    session::{
        agent_profile_catalog::{
            SESSION_AGENT_PROFILE_CATALOG_ID, SessionAgentProfileCatalogHandle,
        },
        session_context::{SESSION_CONTEXT_ID, SessionContext},
        skill_catalog::{SESSION_SKILL_CATALOG_ID, SessionSkillCatalogHandle},
        tool_policy::{SESSION_TOOL_POLICY_ID, SessionToolPolicyHandle},
        workspace_context::{SESSION_WORKSPACE_CONTEXT_ID, SessionWorkspaceContextHandle},
    },
    tool::ToolSource,
    wire::contract::{WIRE_SERVICE_ID, WireServiceHandle},
};

use super::{
    ACTIVE_TOOLS_MODEL, AGENT_PROFILE_SERVICE_ID, AgentConfigData, AgentProfileServiceContract,
    AgentProfileServiceHandle, ApplyProfileOptions, BindAgentInput, ConfigUpdatePayload,
    PROFILE_MODEL, PrepareSystemPromptContextOptions, ProfileBindingSnapshot, ProfileContextDeps,
    ProfileData, ProfileErrorCode, ProfileModelContext, ProfileServiceError, ProfileServiceOptions,
    ProfileSetModelResult, ProfileUpdateData, ResolvedAgentProfile, SystemPromptContext,
    config_update, create_profile_error, ensure_profile_wire_registered,
    prepare_system_prompt_context, profile_bind, reset_active_tools, set_active_tools,
};

#[derive(Default)]
struct MutableState {
    options: ProfileServiceOptions,
    active_tool_names_overlay: Option<Vec<String>>,
    agents_md_warning: Option<String>,
    active_profile: Option<Arc<AgentProfile>>,
    emitted_thinking_effort_warnings: HashSet<String>,
    emitted_tool_pattern_warnings: HashSet<String>,
}

pub struct AgentProfileService {
    wire: WireServiceHandle,
    event_bus: EventBusHandle,
    telemetry: TelemetryServiceHandle,
    telemetry_context: AgentTelemetryContextServiceHandle,
    config: ConfigServiceHandle,
    model_catalog: ModelCatalogHandle,
    protocol_adapters: ProtocolAdapterRegistryHandle,
    env: HostEnvironmentHandle,
    fs: HostFileSystemServiceHandle,
    session_context: Arc<SessionContext>,
    bootstrap: BootstrapServiceHandle,
    workspace: SessionWorkspaceContextHandle,
    catalog: SessionAgentProfileCatalogHandle,
    skill_catalog: SessionSkillCatalogHandle,
    session_tool_policy: SessionToolPolicyHandle,
    tool_registry: AgentToolRegistryServiceHandle,
    builtin_profiles: AgentProfileCatalogHandle,
    state: Mutex<MutableState>,
}

impl AgentProfileService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        wire: WireServiceHandle,
        event_bus: EventBusHandle,
        telemetry: TelemetryServiceHandle,
        telemetry_context: AgentTelemetryContextServiceHandle,
        config: ConfigServiceHandle,
        model_catalog: ModelCatalogHandle,
        protocol_adapters: ProtocolAdapterRegistryHandle,
        env: HostEnvironmentHandle,
        fs: HostFileSystemServiceHandle,
        session_context: Arc<SessionContext>,
        bootstrap: BootstrapServiceHandle,
        workspace: SessionWorkspaceContextHandle,
        catalog: SessionAgentProfileCatalogHandle,
        skill_catalog: SessionSkillCatalogHandle,
        session_tool_policy: SessionToolPolicyHandle,
        tool_registry: AgentToolRegistryServiceHandle,
        builtin_profiles: AgentProfileCatalogHandle,
    ) -> Self {
        ensure_profile_wire_registered();
        Self {
            wire,
            event_bus,
            telemetry,
            telemetry_context,
            config,
            model_catalog,
            protocol_adapters,
            env,
            fs,
            session_context,
            bootstrap,
            workspace,
            catalog,
            skill_catalog,
            session_tool_policy,
            tool_registry,
            builtin_profiles,
            state: Mutex::new(MutableState::default()),
        }
    }

    fn profile_state(&self) -> super::ProfileModelState {
        self.wire.get_model(&PROFILE_MODEL)
    }

    fn active_tool_names(&self) -> Option<Vec<String>> {
        self.state
            .lock()
            .active_tool_names_overlay
            .clone()
            .or_else(|| self.wire.get_model(&ACTIVE_TOOLS_MODEL))
    }

    fn cwd(&self) -> String {
        self.profile_state()
            .cwd
            .or_else(|| self.read_configured_cwd())
            .unwrap_or_default()
    }

    fn model_alias(&self) -> Option<String> {
        self.profile_state().model_alias
    }

    fn profile_name(&self) -> Option<String> {
        self.profile_state().profile_name
    }

    fn thinking_metadata(model: &Model) -> ModelThinkingMetadata {
        ModelThinkingMetadata {
            capabilities: Some(ModelThinkingCapabilities::Structured(
                model.capabilities.clone(),
            )),
            adaptive_thinking: None,
            always_thinking: Some(model.always_thinking),
            support_efforts: model.support_efforts.clone(),
            default_effort: model.default_effort.clone(),
        }
    }

    fn thinking_config(&self) -> Option<ThinkingConfig> {
        self.config
            .get(THINKING_SECTION)
            .and_then(|value| serde_json::from_value(value).ok())
    }

    fn strict_thinking_validation(&self, model: Option<&Model>) -> bool {
        model.is_some_and(|model| {
            requires_strict_thinking_validation(
                self.protocol_adapters.0.as_ref(),
                model.protocol,
                model.provider_type.as_ref().map(|value| value.as_str()),
            )
        })
    }

    fn resolve_thinking_effort(
        &self,
        requested: Option<&str>,
        model: Option<&Model>,
    ) -> ThinkingEffort {
        let config = self.thinking_config();
        let defaults =
            config
                .as_ref()
                .map(|config| crate::kosong::model::types::ThinkingDefaults {
                    enabled: config.enabled,
                    effort: config.effort.clone(),
                });
        let metadata = model.map(Self::thinking_metadata);
        resolve_thinking_effort_for_model(
            requested,
            defaults.as_ref(),
            metadata.as_ref(),
            self.strict_thinking_validation(model),
        )
    }

    fn try_resolve_raw_model(&self) -> Option<Arc<Model>> {
        self.model_alias()
            .and_then(|alias| self.model_catalog.get(&alias).ok())
    }

    fn thinking_level(&self) -> ThinkingEffort {
        let stored = ThinkingEffort::from(self.profile_state().thinking_level);
        let model = self.try_resolve_raw_model();
        if stored.is_off() && model.as_ref().is_some_and(|model| model.always_thinking) {
            self.resolve_thinking_effort(Some(stored.as_str()), model.as_deref())
        } else {
            stored
        }
    }

    fn resolve_thinking_state(
        &self,
        model: Option<&Model>,
    ) -> (ThinkingEffort, Option<ThinkingEffort>) {
        let base = self.thinking_level();
        let forced = resolve_forced_thinking_effort(
            self.thinking_config()
                .as_ref()
                .and_then(|config| config.forced_effort.as_deref()),
            &base,
            drives_thinking_through_traits(
                model.and_then(|model| model.provider_type.as_ref().map(|value| value.as_str())),
            )
            .unwrap_or(false),
        );
        (forced.clone().unwrap_or(base), forced)
    }

    fn assert_bindable(&self, requested: &str) -> Result<(), ProfileServiceError> {
        if let Some(current) = self.profile_name()
            && current != requested
        {
            return Err(Box::new(create_profile_error(
                ProfileErrorCode::ProfileAlreadyBound,
                format!(
                    "agent is already bound to profile \"{current}\"; cannot switch to \"{requested}\" in this session"
                ),
                Some(Map::from_iter([
                    ("current".into(), Value::String(current)),
                    ("requested".into(), Value::String(requested.into())),
                ])),
            )));
        }
        Ok(())
    }

    fn resolve_config_payload(&self, changed: &ProfileUpdateData) -> ConfigUpdatePayload {
        let mut payload = ConfigUpdatePayload {
            cwd: changed.cwd.clone(),
            model_alias: changed.model_alias.clone(),
            profile_name: changed.profile_name.clone(),
            system_prompt: changed.system_prompt.clone(),
            disallowed_tools: changed.disallowed_tools.clone(),
            ..ConfigUpdatePayload::default()
        };
        if changed.thinking_level.is_some() || changed.model_alias.is_some() {
            let alias = changed.model_alias.clone().or_else(|| self.model_alias());
            let model = alias.and_then(|alias| self.model_catalog.get(&alias).ok());
            let current = self.thinking_level();
            let requested = changed
                .thinking_level
                .as_deref()
                .or_else(|| self.model_alias().as_ref().map(|_| current.as_str()));
            payload.thinking_effort =
                Some(self.resolve_thinking_effort(requested, model.as_deref()));
        }
        payload
    }

    fn after_config_dispatch(&self, changed: &ProfileUpdateData) {
        let options = self.state.lock().options.clone();
        if let (Some(cwd), Some(chdir)) = (&changed.cwd, options.chdir) {
            let future = chdir(cwd.clone());
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = future.await;
                });
            }
        }
        if changed.model_alias.is_some() {
            let model = self.try_resolve_raw_model();
            self.telemetry_context.set(AgentTelemetryContextPatch {
                provider_type: Some(model.as_ref().and_then(|model| {
                    model
                        .provider_type
                        .as_ref()
                        .map(ToString::to_string)
                        .or_else(|| Some(model.protocol.to_string()))
                })),
                protocol: Some(model.as_ref().map(|model| model.protocol.to_string())),
                ..AgentTelemetryContextPatch::default()
            });
        }
        if changed.model_alias.is_some() || changed.thinking_level.is_some() {
            self.warn_about_anthropic_thinking_effort();
        }
        self.emit_status_updated(changed.model_alias.is_some() || changed.thinking_level.is_some());
    }

    fn warn_about_anthropic_thinking_effort(&self) {
        let Some(model) = self.try_resolve_raw_model() else {
            return;
        };
        if model.protocol.to_string() != "anthropic" {
            return;
        }
        let Ok(effort) = self.get_effective_thinking_level() else {
            return;
        };
        if effort.as_str() == "on" {
            return;
        }
        let (code, message, known_efforts) = if effort.is_off() {
            if !model.always_thinking {
                return;
            }
            (
                "anthropic-thinking-cannot-disable",
                format!(
                    "Model \"{}\" declares always-on thinking. The configured effort \"off\" will be sent unchanged to the Anthropic-compatible backend.",
                    model.name
                ),
                String::new(),
            )
        } else {
            let efforts = model
                .support_efforts
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter(|value| !value.is_empty())
                .cloned()
                .collect::<Vec<_>>();
            if efforts.is_empty() || efforts.iter().any(|known| known == effort.as_str()) {
                return;
            }
            let known = efforts.join(",");
            (
                "anthropic-thinking-effort-not-listed",
                format!(
                    "Thinking effort \"{effort}\" is not listed for model \"{}\" (known: {}). The configured value will be sent unchanged to the Anthropic-compatible backend.",
                    model.name,
                    efforts.join(", ")
                ),
                known,
            )
        };
        let key = format!(
            "{code}\0{}\0{}\0{effort}\0{known_efforts}",
            model.id, model.name
        );
        if !self
            .state
            .lock()
            .emitted_thinking_effort_warnings
            .insert(key)
        {
            return;
        }
        self.event_bus.publish(DomainEvent::new(
            "warning",
            Map::from_iter([
                ("code".into(), Value::String(code.into())),
                ("message".into(), Value::String(message)),
            ]),
        ));
    }

    fn set_active_tools(&self, names: Option<Vec<String>>) -> Result<(), ProfileServiceError> {
        self.state.lock().active_tool_names_overlay = None;
        let op = match names {
            Some(names) => set_active_tools(names)?,
            None => reset_active_tools()?,
        };
        self.wire.dispatch([op])?;
        Ok(())
    }

    fn emit_status_updated(&self, include_thinking_effort: bool) {
        if let Some(custom) = self
            .state
            .lock()
            .options
            .emit_status_updated
            .clone()
        {
            custom();
            return;
        }
        if !self.has_model() {
            return;
        }
        let mut fields = Map::new();
        if let Some(alias) = self.model_alias() {
            fields.insert("model".into(), Value::String(alias));
        }
        if include_thinking_effort && let Ok(effort) = self.get_effective_thinking_level() {
            fields.insert("thinkingEffort".into(), Value::String(effort.to_string()));
        }
        if let Ok(capabilities) = self.get_model_capabilities() {
            fields.insert(
                "maxContextTokens".into(),
                Value::from(capabilities.max_context_tokens),
            );
        }
        self.event_bus
            .publish(DomainEvent::new("agent.status.updated", fields));
    }

    async fn build_system_prompt_context(
        &self,
        profile: &AgentProfile,
        cwd: Option<&str>,
        options: Option<&ApplyProfileOptions>,
    ) -> Result<AgentProfileContext, ProfileServiceError> {
        let effective_cwd = cwd.unwrap_or(&self.session_context.cwd);
        let additional_dirs = options
            .and_then(|options| options.additional_dirs.clone())
            .unwrap_or_else(|| {
                self.workspace
                    .additional_dirs()
                    .into_iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect()
            });
        let home_dir = PathBuf::from(self.env.home_dir()?);
        let mut context = prepare_system_prompt_context(
            &ProfileContextDeps {
                fs: self.fs.clone(),
                home_dir,
            },
            PathBuf::from(effective_cwd).as_path(),
            Some(self.bootstrap.home_dir()),
            Some(&PrepareSystemPromptContextOptions {
                additional_dirs: Some(additional_dirs),
            }),
        )
        .await;
        context.cwd = Some(effective_cwd.into());
        context.os_kind = Some(self.env.os_kind()?);
        context.shell_name = Some(self.env.shell_name()?.as_str().into());
        context.shell_path = Some(self.env.shell_path()?);
        context.now = Some(chrono::Utc::now().to_rfc3339());
        context.skills = Some(self.resolve_skill_listing().await);
        context.skill_active = Some(self.is_tool_active_for_profile(profile, "Skill"));
        Ok(context)
    }

    fn is_tool_active_for_profile(&self, profile: &AgentProfile, name: &str) -> bool {
        let profile_policy = ToolActivationPolicy {
            tools: profile.tools.clone(),
            disallowed_tools: profile.disallowed_tools.clone(),
        };
        let global = self
            .config
            .get(TOOLS_SECTION)
            .and_then(|value| serde_json::from_value::<ToolsConfig>(value).ok())
            .map(|config| GlobalToolsPolicy {
                enabled: config.enabled,
                disabled: config.disabled,
            });
        let disabled = self.session_tool_policy.disabled_tools();
        is_tool_active_composed(
            ToolPolicyLayers {
                profile: &profile_policy,
                global: global.as_ref(),
                session_disabled_tools: Some(&disabled),
            },
            name,
            ToolSource::Builtin,
        )
    }

    async fn resolve_skill_listing(&self) -> String {
        if self.skill_catalog.ready().await.is_err() {
            return String::new();
        }
        self.skill_catalog.catalog().get_model_skill_listing()
    }

    fn read_configured_cwd(&self) -> Option<String> {
        self.state
            .lock()
            .options
            .cwd
            .as_ref()
            .and_then(|cwd| match cwd {
                super::ProfileCwd::Value(value) => Some(value.clone()),
                super::ProfileCwd::Provider(provider) => provider(),
            })
    }

    fn cache_and_publish_agents_md_warning(&self, context: &AgentProfileContext) {
        let warning = context.agents_md_warning.clone();
        self.state.lock().agents_md_warning = warning.clone();
        if let Some(message) = warning {
            self.event_bus.publish(DomainEvent::new(
                "warning",
                Map::from_iter([
                    ("message".into(), Value::String(message)),
                    ("code".into(), Value::String("agents-md-oversized".into())),
                ]),
            ));
        }
    }

    fn describe_inactive_tool_pattern(
        context: &str,
        field: &str,
        issue: &InactiveToolPattern,
    ) -> String {
        match issue.kind {
            InactiveToolPatternKind::UnknownTool => format!(
                "Tool pattern \"{}\" in {context} {field} does not match any registered or built-in tool; it will never activate anything.",
                issue.pattern
            ),
            InactiveToolPatternKind::WildcardNotMcp => format!(
                "Tool pattern \"{}\" in {context} {field} uses wildcards, which only match MCP tools (names starting with \"mcp__\"); it will never activate anything.",
                issue.pattern
            ),
            InactiveToolPatternKind::IncompleteMcpName => format!(
                "Tool pattern \"{}\" in {context} {field} matches no tool; use \"{}__*\" to match the whole MCP server.",
                issue.pattern, issue.pattern
            ),
        }
    }

    fn publish_tool_pattern_warnings(&self, profile: Option<&AgentProfile>) {
        let mut known = self
            .tool_registry
            .list_references()
            .into_iter()
            .map(|reference| reference.name)
            .collect::<HashSet<_>>();
        for builtin in self.builtin_profiles.list() {
            let patterns = builtin
                .tools
                .iter()
                .flatten()
                .chain(builtin.disallowed_tools.iter().flatten())
                .cloned()
                .collect::<Vec<_>>();
            known.extend(literal_tool_names(&patterns));
        }
        let mut checks: Vec<(String, &'static str, Vec<String>)> = Vec::new();
        if let Some(profile) = profile {
            if let Some(patterns) = profile.tools.clone() {
                checks.push((format!("profile \"{}\"", profile.name), "tools", patterns));
            }
            if let Some(patterns) = profile.disallowed_tools.clone() {
                checks.push((
                    format!("profile \"{}\"", profile.name),
                    "disallowedTools",
                    patterns,
                ));
            }
        }
        if let Some(global) = self
            .config
            .get(TOOLS_SECTION)
            .and_then(|value| serde_json::from_value::<ToolsConfig>(value).ok())
        {
            if let Some(patterns) = global.enabled {
                checks.push(("the global [tools] config".into(), "enabled", patterns));
            }
            if let Some(patterns) = global.disabled {
                checks.push(("the global [tools] config".into(), "disabled", patterns));
            }
        }
        for (context, field, patterns) in checks {
            let is_known = |name: &str| known.contains(name);
            for issue in find_inactive_tool_patterns(&patterns, Some(&is_known)) {
                let key = format!("{context}|{field}|{}", issue.pattern);
                if !self
                    .state
                    .lock()
                    .emitted_tool_pattern_warnings
                    .insert(key)
                {
                    continue;
                }
                self.event_bus.publish(DomainEvent::new(
                    "warning",
                    Map::from_iter([
                        ("code".into(), Value::String("tool-pattern-no-match".into())),
                        (
                            "message".into(),
                            Value::String(Self::describe_inactive_tool_pattern(
                                &context, field, &issue,
                            )),
                        ),
                    ]),
                ));
            }
        }
    }
}

#[async_trait]
impl AgentProfileServiceContract for AgentProfileService {
    fn configure(&self, options: ProfileServiceOptions) {
        let mut state = self.state.lock();
        if options.cwd.is_some() {
            state.options.cwd = options.cwd;
        }
        if options.chdir.is_some() {
            state.options.chdir = options.chdir;
        }
        if options.emit_status_updated.is_some() {
            state.options.emit_status_updated = options.emit_status_updated;
        }
    }

    fn update(&self, changed: ProfileUpdateData) -> Result<(), ProfileServiceError> {
        if changed.profile_name.as_ref() != self.profile_name().as_ref() {
            self.state.lock().active_profile = None;
        }
        let has_config = changed.cwd.is_some()
            || changed.model_alias.is_some()
            || changed.profile_name.is_some()
            || changed.thinking_level.is_some()
            || changed.system_prompt.is_some()
            || changed.disallowed_tools.is_some();
        if has_config {
            self.wire
                .dispatch([config_update(self.resolve_config_payload(&changed))?])?;
            self.after_config_dispatch(&changed);
        }
        if changed.active_tool_names.is_some() {
            self.set_active_tools(changed.active_tool_names)?;
        }
        Ok(())
    }

    fn apply_binding_snapshot(
        &self,
        snapshot: ProfileBindingSnapshot,
    ) -> Result<(), ProfileServiceError> {
        {
            let mut state = self.state.lock();
            state.active_profile = None;
            state.active_tool_names_overlay = None;
        }
        self.wire
            .dispatch([profile_bind(super::ProfileBindPayload {
                cwd: Some(snapshot.cwd.clone()),
                model_alias: snapshot.model_alias.clone(),
                profile_name: snapshot.profile_name.clone(),
                thinking_effort: ThinkingEffort::from(snapshot.thinking_level.clone()),
                system_prompt: snapshot.system_prompt.clone(),
                active_tool_names: snapshot.active_tool_names,
                disallowed_tools: snapshot.disallowed_tools.clone().unwrap_or_default(),
                subagents: snapshot.subagents,
            })?])?;
        self.after_config_dispatch(&ProfileUpdateData {
            cwd: Some(snapshot.cwd),
            model_alias: snapshot.model_alias,
            profile_name: snapshot.profile_name,
            thinking_level: Some(snapshot.thinking_level),
            system_prompt: Some(snapshot.system_prompt),
            disallowed_tools: snapshot.disallowed_tools,
            active_tool_names: None,
        });
        Ok(())
    }

    async fn bind(&self, input: BindAgentInput) -> Result<(), ProfileServiceError> {
        self.catalog.ready().await?;
        self.assert_bindable(&input.profile)?;
        let profile = self.catalog.get(&input.profile).ok_or_else(|| {
            let available = self
                .catalog
                .list()
                .into_iter()
                .map(|profile| profile.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            Box::new(create_profile_error(
                ProfileErrorCode::ProfileUnknown,
                format!(
                    "Unknown agent profile: \"{}\". Available profiles: {available}",
                    input.profile
                ),
                Some(Map::from_iter([
                    ("profile".into(), Value::String(input.profile.clone())),
                    ("available".into(), Value::String(available)),
                ])),
            )) as ProfileServiceError
        })?;
        let alias = input
            .model
            .or_else(|| {
                self.config
                    .get("defaultModel")
                    .and_then(|value| value.as_str().map(str::to_owned))
            })
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                Box::new(create_profile_error(
                    ProfileErrorCode::ModelNotConfigured,
                    format!(
                        "model is required to bind profile \"{}\" (no default model configured)",
                        input.profile
                    ),
                    None,
                )) as ProfileServiceError
            })?;
        let model = self.model_catalog.get(&alias)?;
        if input.strict_thinking == Some(true)
            && let Some(requested) = input.thinking.as_deref()
        {
            let normalized = normalize_requested_thinking_effort(Some(requested));
            let metadata = Self::thinking_metadata(&model);
            if normalized.as_ref().is_some_and(|effort| {
                !model_supports_thinking_effort(
                    effort,
                    Some(&metadata),
                    self.strict_thinking_validation(Some(&model)),
                )
            }) {
                let efforts = model.support_efforts.clone().unwrap_or_default();
                let supported = if efforts.is_empty() {
                    "off".into()
                } else {
                    format!("off, {}", efforts.join(", "))
                };
                return Err(Box::new(create_profile_error(
                    ProfileErrorCode::ModelConfigInvalid,
                    format!(
                        "Thinking effort \"{requested}\" is not supported by model \"{alias}\". Supported efforts: {supported}."
                    ),
                    None,
                )));
            }
        }
        self.session_tool_policy.ready().await?;
        let context = self
            .build_system_prompt_context(&profile, input.cwd.as_deref(), None)
            .await?;
        self.assert_bindable(&profile.name)?;
        let current_profile = self.profile_name();
        let persisted_thinking = current_profile
            .as_ref()
            .map(|_| self.thinking_level().to_string());
        let requested = input.thinking.as_deref().or(persisted_thinking.as_deref());
        let thinking = self.resolve_thinking_effort(requested, Some(&model));
        let system_prompt = profile.render_system_prompt(&context);
        {
            let mut state = self.state.lock();
            state.active_profile = Some(Arc::clone(&profile));
            state.active_tool_names_overlay = None;
        }
        self.wire
            .dispatch([profile_bind(super::ProfileBindPayload {
                cwd: input.cwd.clone(),
                model_alias: Some(alias.clone()),
                profile_name: Some(profile.name.clone()),
                thinking_effort: thinking.clone(),
                system_prompt: system_prompt.clone(),
                active_tool_names: profile.tools.clone(),
                disallowed_tools: profile.disallowed_tools.clone().unwrap_or_default(),
                subagents: profile.subagents.clone(),
            })?])?;
        self.after_config_dispatch(&ProfileUpdateData {
            cwd: input.cwd,
            model_alias: Some(alias),
            profile_name: Some(profile.name.clone()),
            thinking_level: Some(thinking.to_string()),
            system_prompt: Some(system_prompt),
            disallowed_tools: profile.disallowed_tools.clone(),
            active_tool_names: None,
        });
        self.cache_and_publish_agents_md_warning(&context);
        self.publish_tool_pattern_warnings(Some(&profile));
        Ok(())
    }

    async fn set_model(&self, model: String) -> Result<ProfileSetModelResult, ProfileServiceError> {
        let resolved = self.model_catalog.get(&model)?;
        if self.profile_name().is_none() {
            self.bind(BindAgentInput {
                profile: DEFAULT_AGENT_PROFILE_NAME.into(),
                model: Some(model.clone()),
                thinking: None,
                strict_thinking: None,
                cwd: None,
            })
            .await?;
        } else if self.model_alias().as_deref() != Some(&model) {
            self.update(ProfileUpdateData {
                model_alias: Some(model.clone()),
                ..ProfileUpdateData::default()
            })?;
        }
        self.telemetry.track(
            "model_switch",
            Some(&crate::app::telemetry::TelemetryProperties::from([(
                "model".into(),
                Some(Value::String(model.clone())),
            )])),
        );
        Ok(ProfileSetModelResult {
            model,
            provider_name: Some(resolved.provider_name.clone()),
        })
    }

    fn set_thinking(&self, level: String) -> Result<(), ProfileServiceError> {
        if let Some(model) = self.try_resolve_raw_model() {
            let normalized = normalize_requested_thinking_effort(Some(&level));
            let metadata = Self::thinking_metadata(&model);
            if normalized.as_ref().is_some_and(|effort| {
                !model_supports_thinking_effort(
                    effort,
                    Some(&metadata),
                    self.strict_thinking_validation(Some(&model)),
                )
            }) {
                return Err(Box::new(create_profile_error(
                    ProfileErrorCode::ModelConfigInvalid,
                    format!(
                        "Thinking effort \"{level}\" is not supported by model \"{}\".",
                        self.model_alias().unwrap_or_default()
                    ),
                    None,
                )));
            }
        }
        self.update(ProfileUpdateData {
            thinking_level: Some(
                normalize_requested_thinking_effort(Some(&level))
                    .map_or(level, |effort| effort.to_string()),
            ),
            ..ProfileUpdateData::default()
        })
    }

    fn get_model(&self) -> Result<String, ProfileServiceError> {
        self.model_alias().ok_or_else(|| {
            Box::new(crate::_base::errors::errors::Error2::new(
                "model.not_configured",
                "Model not set",
            )) as ProfileServiceError
        })
    }

    fn use_profile(
        &self,
        profile: ResolvedAgentProfile,
        context: SystemPromptContext,
    ) -> Result<(), ProfileServiceError> {
        self.state.lock().active_profile = Some(Arc::new(profile.clone()));
        self.update(ProfileUpdateData {
            profile_name: Some(profile.name.clone()),
            system_prompt: Some(profile.render_system_prompt(&context)),
            disallowed_tools: profile.disallowed_tools.clone(),
            ..ProfileUpdateData::default()
        })?;
        self.set_active_tools(profile.tools)
    }

    async fn apply_profile(
        &self,
        profile: ResolvedAgentProfile,
        options: Option<ApplyProfileOptions>,
    ) -> Result<(), ProfileServiceError> {
        let context = self
            .build_system_prompt_context(&profile, None, options.as_ref())
            .await?;
        self.use_profile(profile, context.clone())?;
        self.cache_and_publish_agents_md_warning(&context);
        let active = self.state.lock().active_profile.clone();
        self.publish_tool_pattern_warnings(active.as_deref());
        Ok(())
    }

    async fn refresh_system_prompt(&self) {
        let profile = {
            let state = self.state.lock();
            state.active_profile.clone()
        }
        .or_else(|| self.profile_name().and_then(|name| self.catalog.get(&name)));
        let Some(profile) = profile else {
            return;
        };
        match self
            .build_system_prompt_context(&profile, Some(&self.cwd()), None)
            .await
        {
            Ok(context) => {
                let _ = self.update(ProfileUpdateData {
                    profile_name: Some(profile.name.clone()),
                    system_prompt: Some(profile.render_system_prompt(&context)),
                    ..ProfileUpdateData::default()
                });
                self.state.lock().active_profile = Some(profile);
                self.cache_and_publish_agents_md_warning(&context);
            }
            Err(error) => self.event_bus.publish(DomainEvent::new(
                "warning",
                Map::from_iter([
                    (
                        "message".into(),
                        Value::String(format!("System prompt refresh skipped: {error}")),
                    ),
                    (
                        "code".into(),
                        Value::String("system-prompt-refresh-failed".into()),
                    ),
                ]),
            )),
        }
    }

    fn get_agents_md_warning(&self) -> Option<String> {
        self.state.lock().agents_md_warning.clone()
    }

    fn data(&self) -> Result<ProfileData, ProfileServiceError> {
        let state = self.profile_state();
        let model = self.try_resolve_raw_model();
        Ok(ProfileData {
            config: AgentConfigData {
                cwd: self.cwd(),
                model_alias: state.model_alias,
                model_capabilities: model
                    .as_ref()
                    .map(|model| model.capabilities.clone())
                    .unwrap_or_else(|| UNKNOWN_CAPABILITY.clone()),
                profile_name: state.profile_name,
                thinking_level: self.thinking_level().to_string(),
                system_prompt: state.system_prompt,
            },
            active_tool_names: self.active_tool_names(),
            disallowed_tools: state.disallowed_tools,
            subagents: state.subagents,
        })
    }

    fn get_effective_thinking_level(&self) -> Result<ThinkingEffort, ProfileServiceError> {
        Ok(self
            .resolve_thinking_state(self.try_resolve_raw_model().as_deref())
            .0)
    }

    fn resolve_model_context(&self) -> Result<ProfileModelContext, ProfileServiceError> {
        let alias = self.get_model()?;
        let model = self.model_catalog.get(&alias)?;
        let loop_control = self
            .config
            .get(LOOP_CONTROL_SECTION)
            .and_then(|value| serde_json::from_value::<LoopControl>(value).ok());
        Ok(ProfileModelContext {
            model_alias: alias,
            model_capabilities: model.capabilities.clone(),
            max_output_size: model.max_output_size,
            always_thinking: model.always_thinking.then_some(true),
            thinking_level: self.resolve_thinking_state(Some(&model)).0,
            reserved_context_size: loop_control
                .as_ref()
                .and_then(|control| control.reserved_context_size),
            compaction_trigger_ratio: loop_control
                .as_ref()
                .and_then(|control| control.compaction_trigger_ratio),
        })
    }

    fn resolve_request_params(&self) -> Result<ModelRequestParams, ProfileServiceError> {
        let model = self.try_resolve_raw_model();
        let thinking = self.resolve_thinking_state(model.as_deref()).0;
        let config = self.thinking_config();
        let overrides = self
            .config
            .get("modelOverrides")
            .and_then(|value| serde_json::from_value::<ModelOverrides>(value).ok())
            .unwrap_or_default();
        let sampling = (overrides.temperature.is_some() || overrides.top_p.is_some()).then_some(
            SamplingOptions {
                temperature: overrides.temperature,
                top_p: overrides.top_p,
            },
        );
        Ok(ModelRequestParams {
            cache_key: Some(self.session_context.session_id.clone()),
            sampling,
            thinking_effort: Some(thinking.clone()),
            thinking_keep: resolve_thinking_keep(
                overrides.thinking_keep.as_deref(),
                config.as_ref().and_then(|config| config.keep.as_deref()),
                &thinking,
            ),
            ..ModelRequestParams::default()
        })
    }

    fn get_model_capabilities(
        &self,
    ) -> Result<crate::kosong::contract::capability::ModelCapability, ProfileServiceError> {
        Ok(self
            .try_resolve_raw_model()
            .map(|model| model.capabilities.clone())
            .unwrap_or_else(|| UNKNOWN_CAPABILITY.clone()))
    }

    fn get_max_output_size(&self) -> Result<Option<u64>, ProfileServiceError> {
        Ok(self
            .try_resolve_raw_model()
            .and_then(|model| model.max_output_size))
    }

    fn has_model(&self) -> bool {
        self.model_alias().is_some()
    }

    fn is_runnable(&self) -> bool {
        self.profile_name().is_some() && self.has_model()
    }

    fn has_provider(&self) -> bool {
        self.try_resolve_raw_model().is_some()
    }

    fn get_system_prompt(&self) -> String {
        self.profile_state().system_prompt
    }

    fn get_active_tool_names(&self) -> Option<Vec<String>> {
        self.active_tool_names()
    }

    fn add_active_tool(&self, name: String) {
        let Some(mut names) = self.active_tool_names() else {
            return;
        };
        if !names.contains(&name) {
            names.push(name);
            self.state.lock().active_tool_names_overlay = Some(names);
        }
    }

    fn remove_active_tool(&self, name: &str) {
        let Some(mut names) = self.active_tool_names() else {
            return;
        };
        let previous = names.len();
        names.retain(|candidate| candidate != name);
        if names.len() != previous {
            self.state.lock().active_tool_names_overlay = Some(names);
        }
    }
}

pub fn register_agent_profile_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_PROFILE_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service = AgentProfileService::new(
                (*accessor.get(WIRE_SERVICE_ID)?).clone(),
                (*accessor.get(EVENT_BUS_SERVICE_ID)?).clone(),
                (*accessor.get(TELEMETRY_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_TELEMETRY_CONTEXT_SERVICE_ID)?).clone(),
                (*accessor.get(CONFIG_SERVICE_ID)?).clone(),
                (*accessor.get(MODEL_CATALOG_SERVICE_ID)?).clone(),
                (*accessor.get(PROTOCOL_ADAPTER_REGISTRY_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_ENVIRONMENT_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?).clone(),
                accessor.get(SESSION_CONTEXT_ID)?,
                (*accessor.get(BOOTSTRAP_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_WORKSPACE_CONTEXT_ID)?).clone(),
                (*accessor.get(SESSION_AGENT_PROFILE_CATALOG_ID)?).clone(),
                (*accessor.get(SESSION_SKILL_CATALOG_ID)?).clone(),
                (*accessor.get(SESSION_TOOL_POLICY_ID)?).clone(),
                (*accessor.get(AGENT_TOOL_REGISTRY_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_PROFILE_CATALOG_SERVICE_ID)?).clone(),
            );
            Ok(AgentProfileServiceHandle(Arc::new(service)))
        }),
        InstantiationType::Eager,
        "profile",
    );
}
