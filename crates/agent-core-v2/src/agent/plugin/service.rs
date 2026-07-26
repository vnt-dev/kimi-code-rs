//! Main-agent plugin session-start guidance.
//!
//! Original:
//! `packages/agent-core-v2/src/agent/plugin/agentPluginService.ts`.

use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use serde_json::{Map, Value};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        lifecycle::lifecycle_machine::BoxError,
        log::{LOG_SERVICE_ID, LogPayload, LogServiceHandle, Logger},
        utils::xml_escape::escape_xml_attr,
    },
    agent::{
        context_injector::{
            AGENT_CONTEXT_INJECTOR_SERVICE_ID, AgentContextInjectorServiceHandle,
            ContextInjectionContent,
        },
        context_memory::{
            AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceHandle, ContextMessage,
            PromptOrigin,
        },
        scope_context::{AGENT_SCOPE_CONTEXT_ID, AgentScopeContext},
        system_reminder::{AGENT_SYSTEM_REMINDER_SERVICE_ID, AgentSystemReminderServiceHandle},
    },
    app::{
        plugin::{EnabledPluginSessionStart, PLUGIN_SERVICE_ID, PluginServiceHandle},
        skill_catalog::{SkillCatalogContract, SkillDefinition},
    },
    session::{
        session_context::{SESSION_CONTEXT_ID, SessionContext},
        skill_catalog::{
            PLUGIN_SKILL_SOURCE_ID, SESSION_SKILL_CATALOG_ID, SessionSkillCatalogHandle,
        },
    },
};

use super::{AGENT_PLUGIN_SERVICE_ID, AgentPluginServiceContract, AgentPluginServiceHandle};

pub const SESSION_START_INJECTION_VARIANT: &str = "plugin_session_start";
const MAIN_AGENT_ID: &str = "main";
const SUPERSEDES_SUFFIX: &str =
    "This supersedes any earlier plugin_session_start reminder in this session.";
const NEUTRAL_REMINDER: &str = "There are currently no active plugin session starts. This supersedes any earlier plugin_session_start reminder in this session.";

pub struct AgentPluginService {
    reminders: AgentSystemReminderServiceHandle,
    context: AgentContextMemoryServiceHandle,
    plugins: PluginServiceHandle,
    skill_catalog: SessionSkillCatalogHandle,
    session_context: SessionContext,
    log: LogServiceHandle,
    disposables: DisposableStore,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl AgentPluginService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scope_context: AgentScopeContext,
        injector: AgentContextInjectorServiceHandle,
        reminders: AgentSystemReminderServiceHandle,
        context: AgentContextMemoryServiceHandle,
        plugins: PluginServiceHandle,
        skill_catalog: SessionSkillCatalogHandle,
        session_context: SessionContext,
        log: LogServiceHandle,
    ) -> Arc<Self> {
        let service = Arc::new(Self {
            reminders,
            context,
            plugins,
            skill_catalog,
            session_context,
            log,
            disposables: DisposableStore::new(),
            tasks: Mutex::new(Vec::new()),
        });
        if scope_context.agent_id == MAIN_AGENT_ID {
            service.install(injector);
        }
        service
    }

    fn install(self: &Arc<Self>, injector: AgentContextInjectorServiceHandle) {
        let weak = Arc::downgrade(self);
        self.disposables.add(injector.register(
            SESSION_START_INJECTION_VARIANT.into(),
            Arc::new(move |context| {
                let weak = weak.clone();
                Box::pin(async move {
                    if !context.injected_positions.is_empty() {
                        return Ok(None);
                    }
                    let Some(service) = weak.upgrade() else {
                        return Ok(None);
                    };
                    Ok(service
                        .render_session_start_reminder()
                        .await?
                        .map(ContextInjectionContent::Text))
                })
                    as BoxFuture<'static, Result<Option<ContextInjectionContent>, BoxError>>
            }),
        ));

        let weak = Arc::downgrade(self);
        self.disposables.add(
            self.skill_catalog
                .on_did_change()
                .subscribe(move |source_id| {
                    if source_id != PLUGIN_SKILL_SOURCE_ID {
                        return;
                    }
                    let Some(service) = weak.upgrade() else {
                        return;
                    };
                    service.spawn_fresh_reminder();
                }),
        );
    }

    fn spawn_fresh_reminder(self: &Arc<Self>) {
        let service = Arc::clone(self);
        let task = tokio::spawn(async move {
            let _ = service.append_fresh_session_start_reminder().await;
        });
        self.tasks.lock().unwrap().push(task);
    }

    async fn render_session_start_reminder(&self) -> Result<Option<String>, BoxError> {
        let session_starts = self.plugins.enabled_session_starts().await?;
        if session_starts.is_empty() {
            return Ok(None);
        }
        self.skill_catalog
            .ready()
            .await
            .map_err(|error| Box::new(error) as BoxError)?;
        let catalog = self.skill_catalog.catalog();
        Ok(render_plugin_session_start_reminder(
            &session_starts,
            Some(catalog.as_ref()),
            Some(self.log.0.as_ref()),
            Some(&self.session_context.session_id),
        ))
    }

    pub async fn append_fresh_session_start_reminder(&self) -> Result<(), BoxError> {
        if let Some(reminder) = self.render_session_start_reminder().await? {
            self.reminders
                .append_system_reminder(
                    &format!("{reminder}\n\n{SUPERSEDES_SUFFIX}"),
                    PromptOrigin::Injection {
                        variant: SESSION_START_INJECTION_VARIANT.into(),
                    },
                )
                .map_err(|error| Box::new(error) as BoxError)?;
        } else if should_neutralize_plugin_session_start(&self.context.get()) {
            self.reminders
                .append_system_reminder(
                    NEUTRAL_REMINDER,
                    PromptOrigin::Injection {
                        variant: SESSION_START_INJECTION_VARIANT.into(),
                    },
                )
                .map_err(|error| Box::new(error) as BoxError)?;
        }
        Ok(())
    }
}

impl AgentPluginServiceContract for AgentPluginService {}

impl Disposable for AgentPluginService {
    fn dispose(&self) -> DisposeResult {
        let result = self.disposables.dispose();
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
        result
    }
}

pub fn render_plugin_session_start_reminder(
    session_starts: &[EnabledPluginSessionStart],
    catalog: Option<&dyn SkillCatalogContract>,
    log: Option<&dyn Logger>,
    session_id: Option<&str>,
) -> Option<String> {
    if session_starts.is_empty() {
        return None;
    }
    let catalog = catalog?;
    let mut blocks = Vec::new();
    for session_start in session_starts {
        let Some(skill) =
            catalog.get_plugin_skill(&session_start.plugin_id, &session_start.skill_name)
        else {
            if let Some(log) = log {
                log.warn(
                    "plugin sessionStart skill not found",
                    Some(LogPayload::Context(Map::from_iter([
                        (
                            "pluginId".into(),
                            Value::String(session_start.plugin_id.clone()),
                        ),
                        (
                            "skillName".into(),
                            Value::String(session_start.skill_name.clone()),
                        ),
                    ]))),
                );
            }
            continue;
        };
        let skill_content = catalog.render_skill_prompt(&skill, "", session_id);
        blocks.push(render_session_start_block(
            session_start,
            &skill,
            &skill_content,
        ));
    }
    (!blocks.is_empty()).then(|| blocks.join("\n"))
}

pub fn should_neutralize_plugin_session_start(history: &[ContextMessage]) -> bool {
    history.iter().any(|message| {
        matches!(
            message.origin.as_ref(),
            Some(PromptOrigin::Injection { variant })
                if variant == SESSION_START_INJECTION_VARIANT
        ) || matches!(
            message.origin.as_ref(),
            Some(PromptOrigin::CompactionSummary)
        )
    })
}

fn render_session_start_block(
    session_start: &EnabledPluginSessionStart,
    skill: &SkillDefinition,
    skill_content: &str,
) -> String {
    format!(
        "<plugin_session_start plugin=\"{}\" skill=\"{}\">\n{skill_content}\n</plugin_session_start>",
        escape_xml_attr(&session_start.plugin_id),
        escape_xml_attr(&skill.name)
    )
}

// Original: registerScopedService(Agent, ..., Eager, "agentPlugin").
pub fn register_agent_plugin_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_PLUGIN_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let scope_context = accessor.get(AGENT_SCOPE_CONTEXT_ID)?;
            let injector = accessor.get(AGENT_CONTEXT_INJECTOR_SERVICE_ID)?;
            let reminders = accessor.get(AGENT_SYSTEM_REMINDER_SERVICE_ID)?;
            let context = accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?;
            let plugins = accessor.get(PLUGIN_SERVICE_ID)?;
            let skill_catalog = accessor.get(SESSION_SKILL_CATALOG_ID)?;
            let session_context = accessor.get(SESSION_CONTEXT_ID)?;
            let log = accessor.get(LOG_SERVICE_ID)?;
            let service = AgentPluginService::new(
                (*scope_context).clone(),
                (*injector).clone(),
                (*reminders).clone(),
                (*context).clone(),
                (*plugins).clone(),
                (*skill_catalog).clone(),
                (*session_context).clone(),
                (*log).clone(),
            );
            let service: Arc<dyn AgentPluginServiceContract> = service;
            Ok(AgentPluginServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "agentPlugin",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::{
        _base::di::scope::get_scoped_service_descriptors,
        _base::log::LogContext,
        agent::context_memory::{ContextMessage, PromptOrigin},
        app::skill_catalog::{
            InMemorySkillCatalog, RegisterSkillOptions, SkillMetadata, SkillPluginContext,
            SkillSource,
        },
        kosong::contract::message::{Message, Role},
    };

    #[derive(Default)]
    struct RecordingLogger(Mutex<Vec<(String, Option<LogPayload>)>>);

    impl Logger for RecordingLogger {
        fn error(&self, message: &str, payload: Option<LogPayload>) {
            self.0.lock().unwrap().push((message.into(), payload));
        }

        fn warn(&self, message: &str, payload: Option<LogPayload>) {
            self.0.lock().unwrap().push((message.into(), payload));
        }

        fn info(&self, message: &str, payload: Option<LogPayload>) {
            self.0.lock().unwrap().push((message.into(), payload));
        }

        fn debug(&self, message: &str, payload: Option<LogPayload>) {
            self.0.lock().unwrap().push((message.into(), payload));
        }

        fn child(&self, _context: LogContext) -> Arc<dyn Logger> {
            Arc::new(Self::default())
        }
    }

    fn plugin_skill(plugin_id: &str, name: &str, content: &str) -> SkillDefinition {
        SkillDefinition {
            name: name.into(),
            description: "plugin skill".into(),
            path: format!("/plugins/{plugin_id}/skills/{name}/SKILL.md"),
            dir: format!("/plugins/{plugin_id}/skills/{name}"),
            content: content.into(),
            metadata: SkillMetadata::default(),
            source: SkillSource::Extra,
            plugin: Some(SkillPluginContext {
                id: plugin_id.into(),
                instructions: Some("Always be helpful.".into()),
            }),
            mermaid: None,
            d2: None,
        }
    }

    fn message(origin: PromptOrigin) -> ContextMessage {
        ContextMessage {
            message: Message::new(Role::User, Vec::new(), Vec::new()),
            id: None,
            provider_message_id: None,
            origin: Some(origin),
            is_error: None,
            note: None,
        }
    }

    #[test]
    fn renders_plugin_identity_instructions_session_and_xml_attributes() {
        let mut catalog = InMemorySkillCatalog::default();
        catalog.register(
            plugin_skill("demo&one", "demo\"skill", "Do ${KIMI_SESSION_ID}."),
            RegisterSkillOptions::default(),
        );
        let rendered = render_plugin_session_start_reminder(
            &[EnabledPluginSessionStart {
                plugin_id: "demo&one".into(),
                skill_name: "demo\"skill".into(),
            }],
            Some(&catalog),
            None,
            Some("session-1"),
        )
        .unwrap();
        assert!(rendered.contains("plugin=\"demo&amp;one\" skill=\"demo&quot;skill\""));
        assert!(rendered.contains("<kimi-plugin-instructions plugin=\"demo&amp;one\">"));
        assert!(rendered.contains("Always be helpful."));
        assert!(rendered.contains("Do session-1."));
    }

    #[test]
    fn skips_missing_or_empty_declarations_and_joins_valid_blocks() {
        let mut catalog = InMemorySkillCatalog::default();
        catalog.register(
            plugin_skill("one", "first", "first body"),
            RegisterSkillOptions::default(),
        );
        catalog.register(
            plugin_skill("two", "second", "second body"),
            RegisterSkillOptions::default(),
        );
        assert_eq!(
            render_plugin_session_start_reminder(&[], Some(&catalog), None, None),
            None
        );
        let rendered = render_plugin_session_start_reminder(
            &[
                EnabledPluginSessionStart {
                    plugin_id: "missing".into(),
                    skill_name: "none".into(),
                },
                EnabledPluginSessionStart {
                    plugin_id: "one".into(),
                    skill_name: "first".into(),
                },
                EnabledPluginSessionStart {
                    plugin_id: "two".into(),
                    skill_name: "second".into(),
                },
            ],
            Some(&catalog),
            None,
            None,
        )
        .unwrap();
        assert!(rendered.contains("first body"));
        assert!(rendered.contains("second body"));
        assert!(rendered.contains("</plugin_session_start>\n<plugin_session_start"));
        assert_eq!(
            render_plugin_session_start_reminder(
                &[EnabledPluginSessionStart {
                    plugin_id: "one".into(),
                    skill_name: "first".into()
                }],
                None,
                None,
                None
            ),
            None
        );
    }

    #[test]
    fn missing_skill_warns_with_plugin_and_skill_identity() {
        let catalog = InMemorySkillCatalog::default();
        let log = RecordingLogger::default();
        assert_eq!(
            render_plugin_session_start_reminder(
                &[EnabledPluginSessionStart {
                    plugin_id: "demo".into(),
                    skill_name: "missing".into(),
                }],
                Some(&catalog),
                Some(&log),
                None,
            ),
            None
        );
        let records = log.0.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].0, "plugin sessionStart skill not found");
        let Some(LogPayload::Context(payload)) = &records[0].1 else {
            panic!("warning payload must be structured context");
        };
        assert_eq!(payload["pluginId"], "demo");
        assert_eq!(payload["skillName"], "missing");
    }

    #[test]
    fn neutralizes_only_after_prior_injection_or_compaction() {
        assert!(!should_neutralize_plugin_session_start(&[]));
        assert!(!should_neutralize_plugin_session_start(&[message(
            PromptOrigin::User
        )]));
        assert!(!should_neutralize_plugin_session_start(&[message(
            PromptOrigin::Injection {
                variant: "other".into()
            }
        )]));
        assert!(should_neutralize_plugin_session_start(&[message(
            PromptOrigin::Injection {
                variant: SESSION_START_INJECTION_VARIANT.into()
            }
        )]));
        assert!(should_neutralize_plugin_session_start(&[message(
            PromptOrigin::CompactionSummary
        )]));
    }

    #[test]
    fn registration_is_eager_agent_scoped_with_source_domain() {
        register_agent_plugin_service();
        let descriptors = get_scoped_service_descriptors(LifecycleScope::Agent);
        let descriptor = descriptors
            .iter()
            .find(|entry| entry.id.to_string() == AGENT_PLUGIN_SERVICE_ID.to_string())
            .expect("agent plugin service is registered");
        assert!(!descriptor.descriptor.supports_delayed_instantiation);
        assert_eq!(descriptor.domain, "agentPlugin");
    }
}
