//! Agent-scoped skill activation implementation.
//!
//! Original: `packages/agent-core-v2/src/agent/skill/skillService.ts`.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::Error2,
    },
    agent::{
        context_memory::{
            ContextMessage, SkillActivationOrigin, SkillActivationOriginKind,
            SkillActivationTrigger, SkillSource as ContextSkillSource,
        },
        loop_::{
            TurnHandle,
            errors::{TURN_AGENT_BUSY, ensure_loop_errors_registered},
        },
        prompt::{AGENT_PROMPT_SERVICE_ID, AgentPromptServiceHandle, PromptInput},
    },
    app::{
        skill_catalog::{
            SkillDefinition, SkillSource,
            errors::{SKILL_NOT_FOUND, SKILL_TYPE_UNSUPPORTED, ensure_skill_errors_registered},
            is_user_activatable_skill_type,
        },
        telemetry::{TELEMETRY_SERVICE_ID, TelemetryServiceHandle},
    },
    kosong::contract::message::{ContentPart, Message, Role},
    session::{
        session_context::{SESSION_CONTEXT_ID, SessionContext},
        skill_catalog::{SESSION_SKILL_CATALOG_ID, SessionSkillCatalogHandle},
    },
    wire::contract::{WIRE_SERVICE_ID, WireServiceHandle},
};

use super::{
    AGENT_SKILL_SERVICE_ID, AgentSkillServiceContract, AgentSkillServiceError,
    AgentSkillServiceHandle, RenderSkillPromptInput, SkillActivationInput,
    render_user_slash_skill_prompt, skill_activate,
};

pub struct AgentSkillService {
    skill_catalog: SessionSkillCatalogHandle,
    prompt: AgentPromptServiceHandle,
    wire: WireServiceHandle,
    telemetry: TelemetryServiceHandle,
    session_context: SessionContext,
}

impl AgentSkillService {
    pub fn new(
        skill_catalog: SessionSkillCatalogHandle,
        prompt: AgentPromptServiceHandle,
        wire: WireServiceHandle,
        telemetry: TelemetryServiceHandle,
        session_context: SessionContext,
    ) -> Self {
        ensure_skill_errors_registered();
        ensure_loop_errors_registered();
        Self {
            skill_catalog,
            prompt,
            wire,
            telemetry,
            session_context,
        }
    }

    async fn record_activation(
        &self,
        origin: SkillActivationOrigin,
        input: Option<Vec<ContentPart>>,
    ) -> Result<Option<TurnHandle>, AgentSkillServiceError> {
        self.wire
            .dispatch([skill_activate(origin.clone())?])
            .map_err(|error| Box::new(error) as AgentSkillServiceError)?;
        publish_activation(&self.telemetry, &origin);

        let Some(content) = input else {
            return Ok(None);
        };
        let handle = self
            .prompt
            .enqueue(PromptInput {
                id: None,
                message: ContextMessage {
                    message: Message::new(Role::User, content, Vec::new()),
                    id: None,
                    provider_message_id: None,
                    origin: Some(origin.into()),
                    is_error: None,
                    note: None,
                },
            })
            .await?;
        Ok(handle.launched().await)
    }

    fn render_skill_prompt(&self, skill: &SkillDefinition, raw_args: &str) -> String {
        self.skill_catalog.catalog().render_skill_prompt(
            skill,
            raw_args,
            Some(&self.session_context.session_id),
        )
    }
}

#[async_trait]
impl AgentSkillServiceContract for AgentSkillService {
    async fn activate(
        &self,
        input: SkillActivationInput,
    ) -> Result<TurnHandle, AgentSkillServiceError> {
        self.skill_catalog
            .ready()
            .await
            .map_err(|error| Box::new(error) as AgentSkillServiceError)?;
        let skill = self
            .skill_catalog
            .catalog()
            .get_skill(&input.name)
            .ok_or_else(|| {
                Box::new(Error2::new(
                    SKILL_NOT_FOUND,
                    format!("Skill \"{}\" was not found", input.name),
                )) as AgentSkillServiceError
            })?;
        if !is_user_activatable_skill_type(skill.metadata.kind.as_deref()) {
            return Err(Box::new(Error2::new(
                SKILL_TYPE_UNSUPPORTED,
                format!("Skill \"{}\" cannot be activated by the user", skill.name),
            )));
        }

        let skill_args = input.args.clone().unwrap_or_default();
        let skill_content = self.render_skill_prompt(&skill, &skill_args);
        let content = vec![ContentPart::Text {
            text: render_user_slash_skill_prompt(RenderSkillPromptInput {
                skill_name: &skill.name,
                skill_args: &skill_args,
                skill_content: &skill_content,
                skill_source: Some(skill.source),
                skill_dir: Some(&skill.dir),
            }),
        }];
        let origin = SkillActivationOrigin {
            kind: SkillActivationOriginKind::SkillActivation,
            activation_id: Uuid::new_v4().to_string(),
            skill_name: skill.name.clone(),
            skill_args: input.args,
            trigger: SkillActivationTrigger::UserSlash,
            skill_type: skill.metadata.kind.clone(),
            skill_path: Some(skill.path.clone()),
            skill_source: Some(context_skill_source(skill.source)),
        };

        self.record_activation(origin, Some(content))
            .await?
            .ok_or_else(|| {
                Box::new(Error2::new(
                    TURN_AGENT_BUSY,
                    "Cannot activate skill while another turn is active",
                )) as AgentSkillServiceError
            })
    }

    fn record_model_tool_activation(&self, origin: SkillActivationOrigin) {
        if let Ok(op) = skill_activate(origin.clone()) {
            let _ = self.wire.dispatch([op]);
            publish_activation(&self.telemetry, &origin);
        }
    }
}

impl Disposable for AgentSkillService {
    fn dispose(&self) -> DisposeResult {
        Ok(())
    }
}

fn publish_activation(telemetry: &TelemetryServiceHandle, origin: &SkillActivationOrigin) {
    use crate::app::telemetry::{
        FlowInvokedEvent, SkillInvokedEvent, SkillTrigger, TelemetryServiceEventExt,
    };

    telemetry
        .track_event(&SkillInvokedEvent {
            skill_name: origin.skill_name.clone(),
            trigger: match origin.trigger {
                SkillActivationTrigger::UserSlash => SkillTrigger::UserSlash,
                SkillActivationTrigger::ModelTool => SkillTrigger::ModelTool,
                SkillActivationTrigger::NestedSkill => SkillTrigger::NestedSkill,
            },
        })
        .expect("skill invocation telemetry payload is serializable");
    if origin.skill_type.as_deref() == Some("flow") {
        telemetry
            .track_event(&FlowInvokedEvent {
                flow_name: origin.skill_name.clone(),
            })
            .expect("flow invocation telemetry payload is serializable");
    }
}

fn context_skill_source(source: SkillSource) -> ContextSkillSource {
    match source {
        SkillSource::Project => ContextSkillSource::Project,
        SkillSource::User => ContextSkillSource::User,
        SkillSource::Extra => ContextSkillSource::Extra,
        SkillSource::Builtin => ContextSkillSource::Builtin,
    }
}

// Original: registerScopedService(Agent, ..., Eager, "skill").
pub fn register_agent_skill_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_SKILL_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service = AgentSkillService::new(
                (*accessor.get(SESSION_SKILL_CATALOG_ID)?).clone(),
                (*accessor.get(AGENT_PROMPT_SERVICE_ID)?).clone(),
                (*accessor.get(WIRE_SERVICE_ID)?).clone(),
                (*accessor.get(TELEMETRY_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_CONTEXT_ID)?).clone(),
            );
            let service: Arc<dyn AgentSkillServiceContract> = Arc::new(service);
            Ok(AgentSkillServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "skill",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::di::scope::get_scoped_service_descriptors;

    #[test]
    fn source_and_context_skill_sources_have_a_total_mapping() {
        assert_eq!(
            context_skill_source(SkillSource::Project),
            ContextSkillSource::Project
        );
        assert_eq!(
            context_skill_source(SkillSource::Builtin),
            ContextSkillSource::Builtin
        );
    }

    #[test]
    fn registration_is_eager_agent_scoped_with_source_domain() {
        register_agent_skill_service();
        let descriptors = get_scoped_service_descriptors(LifecycleScope::Agent);
        let descriptor = descriptors
            .iter()
            .find(|entry| entry.id.to_string() == AGENT_SKILL_SERVICE_ID.to_string())
            .expect("skill service is registered");
        assert!(!descriptor.descriptor.supports_delayed_instantiation);
        assert_eq!(descriptor.domain, "skill");
    }
}
