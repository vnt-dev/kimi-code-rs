//! Model-invoked inline skill tool.
//!
//! Original: `packages/agent-core-v2/src/agent/skill/tools/skill.ts`.

use std::{collections::HashMap, sync::Arc, sync::LazyLock};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    _base::{di::instantiation::ServicesAccessorExt, utils::render_prompt::render_prompt},
    agent::{
        context_memory::{
            SkillActivationOrigin, SkillActivationOriginKind, SkillActivationTrigger,
            SkillSource as ContextSkillSource,
        },
        skill::{AGENT_SKILL_SERVICE_ID, AgentSkillServiceHandle},
        tool_registry::{ToolContributionOptions, register_tool},
    },
    app::skill_catalog::{SkillSource, SkillSourceError, is_inline_skill_type},
    kosong::contract::{
        message::{ContentPart, ToolCall},
        tool::Tool,
    },
    session::{
        session_context::{SESSION_CONTEXT_ID, SessionContext},
        skill_catalog::{SESSION_SKILL_CATALOG_ID, SessionSkillCatalogHandle},
    },
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolDelivery, ToolDeliveryKind, ToolDeliveryMessage, ToolExecution, ToolInputDisplay,
        input_schema::to_input_json_schema, rule_match::matches_glob_rule_subject,
    },
};

use super::super::{RenderSkillPromptInput, SkillPromptTrigger, render_model_tool_skill_prompt};

pub const MAX_SKILL_QUERY_DEPTH: usize = 3;

const SKILL_DESCRIPTION_TEMPLATE: &str = include_str!("skill.md");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillToolInput {
    pub skill: String,
    pub args: Option<String>,
}

impl<'de> Deserialize<'de> for SkillToolInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        parse_skill_tool_input(&value).map_err(serde::de::Error::custom)
    }
}

pub fn parse_skill_tool_input(value: &Value) -> Result<SkillToolInput, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "Skill input must be an object".to_owned())?;
    if object.keys().any(|key| key != "skill" && key != "args") {
        return Err("Skill input contains unknown properties".into());
    }
    let skill = object
        .get("skill")
        .and_then(Value::as_str)
        .ok_or_else(|| "skill must be a string".to_owned())?
        .to_owned();
    let args = object
        .get("args")
        .map(|args| {
            args.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "args must be a string".to_owned())
        })
        .transpose()?;
    Ok(SkillToolInput { skill, args })
}

pub static SKILL_TOOL_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "The exact name of the skill to invoke, spelled as it appears in the current skill listing (e.g. \"commit\", \"pdf\")."
                },
                "args": {
                    "type": "string",
                    "description": "Optional argument string for the skill, written like a command line (e.g. `-m \"fix bug\"`, `123`, a file path). It is split on whitespace (quotes group a token) and expanded into the skill's placeholders ($NAME, $1, $ARGUMENTS); if the skill body has no placeholders, the whole string is still appended as a trailing `ARGUMENTS:` line. Omit it only when there is nothing to pass."
                }
            },
            "required": ["skill"],
            "additionalProperties": false
        })
        .as_object()
        .cloned()
        .expect("Skill schema is an object"),
    )
});

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
#[error(
    "Nested skill invocation{label} exceeded the maximum depth of {depth} — refusing to recurse further.",
    label = skill_label(.skill_name.as_deref())
)]
pub struct NestedSkillTooDeepError {
    pub depth: usize,
    pub skill_name: Option<String>,
}

impl NestedSkillTooDeepError {
    pub fn new(depth: usize, skill_name: Option<String>) -> Self {
        Self { depth, skill_name }
    }
}

fn skill_label(skill_name: Option<&str>) -> String {
    skill_name.map_or_else(String::new, |name| format!(" \"{name}\""))
}

#[derive(Debug, thiserror::Error)]
pub enum ExecuteModelSkillError {
    #[error(transparent)]
    NestedSkillTooDeep(#[from] NestedSkillTooDeepError),
    #[error(transparent)]
    Catalog(#[from] SkillSourceError),
}

pub struct SkillTool {
    catalog: SessionSkillCatalogHandle,
    skill: AgentSkillServiceHandle,
    session_context: SessionContext,
    query_depth: usize,
    definition: Tool,
}

impl SkillTool {
    pub fn new(
        catalog: SessionSkillCatalogHandle,
        skill: AgentSkillServiceHandle,
        session_context: SessionContext,
    ) -> Self {
        let variables = HashMap::from([(
            "MAX_SKILL_QUERY_DEPTH".to_owned(),
            Value::from(MAX_SKILL_QUERY_DEPTH),
        )]);
        Self {
            catalog,
            skill,
            session_context,
            query_depth: 0,
            definition: Tool {
                name: "Skill".into(),
                description: render_prompt(
                    SKILL_DESCRIPTION_TEMPLATE.trim_end_matches(['\r', '\n']),
                    &variables,
                ),
                parameters: SKILL_TOOL_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }

    pub fn with_initial_query_depth(&self, initial_query_depth: usize) -> Self {
        let mut clone = Self::new(
            self.catalog.clone(),
            self.skill.clone(),
            self.session_context.clone(),
        );
        clone.query_depth = initial_query_depth;
        clone
    }
}

#[async_trait]
impl ExecutableTool for SkillTool {
    type Input = SkillToolInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, args: SkillToolInput) -> ToolExecution {
        let catalog = self.catalog.clone();
        let skill = self.skill.clone();
        let query_depth = self.query_depth;
        let session_id = self.session_context.session_id.clone();
        let execution_args = args.clone();
        let execute = Arc::new(move |_context: ExecutableToolContext| {
            let catalog = catalog.clone();
            let skill = skill.clone();
            let args = execution_args.clone();
            let session_id = session_id.clone();
            Box::pin(async move {
                execute_model_skill(&catalog, &skill, args, query_depth, &session_id)
                    .await
                    .unwrap_or_else(|error| ExecutableToolResult::error(error.to_string()))
            }) as BoxFuture<'static, ExecutableToolResult>
        });

        let skill_name = args.skill.clone();
        let mut execution = RunnableToolExecution::new("Skill", execute);
        execution.description = Some(format!("Invoke skill {}", args.skill));
        execution.display = Some(ToolInputDisplay::SkillCall {
            skill_name: args.skill,
            args: args.args,
        });
        execution.matches_rule = Some(Arc::new(move |rule_args| {
            matches_glob_rule_subject(rule_args, &skill_name)
        }));
        ToolExecution::Runnable(execution)
    }
}

pub async fn execute_model_skill(
    catalog: &SessionSkillCatalogHandle,
    skill_service: &AgentSkillServiceHandle,
    args: SkillToolInput,
    query_depth: usize,
    session_id: &str,
) -> Result<ExecutableToolResult, ExecuteModelSkillError> {
    if query_depth >= MAX_SKILL_QUERY_DEPTH {
        return Err(NestedSkillTooDeepError::new(MAX_SKILL_QUERY_DEPTH, Some(args.skill)).into());
    }

    catalog.ready().await?;
    let catalog_view = catalog.catalog();
    let Some(skill) = catalog_view.get_skill(&args.skill) else {
        return Ok(ExecutableToolResult::error(format!(
            "Skill \"{}\" not found in the current skill listing.",
            args.skill
        )));
    };
    if skill.metadata.disable_model_invocation == Some(true) {
        return Ok(ExecutableToolResult::error(format!(
            "Skill \"{}\" can only be triggered by the user (model invocation is disabled).",
            args.skill
        )));
    }
    if !is_inline_skill_type(skill.metadata.kind.as_deref()) {
        return Ok(ExecutableToolResult::error(format!(
            "Skill \"{}\" is not an inline skill and cannot be invoked by the model in v1.",
            skill.name
        )));
    }

    let skill_args = args.args.unwrap_or_default();
    let (activation_trigger, prompt_trigger) = if query_depth > 0 {
        (
            SkillActivationTrigger::NestedSkill,
            SkillPromptTrigger::NestedSkill,
        )
    } else {
        (
            SkillActivationTrigger::ModelTool,
            SkillPromptTrigger::ModelTool,
        )
    };
    let origin = SkillActivationOrigin {
        kind: SkillActivationOriginKind::SkillActivation,
        activation_id: Uuid::new_v4().to_string(),
        skill_name: skill.name.clone(),
        skill_args: (!skill_args.is_empty()).then(|| skill_args.clone()),
        trigger: activation_trigger,
        skill_type: skill.metadata.kind.clone(),
        skill_path: Some(skill.path.clone()),
        skill_source: Some(context_skill_source(skill.source)),
    };
    let skill_content = catalog_view.render_skill_prompt(&skill, &skill_args, Some(session_id));
    let prompt = render_model_tool_skill_prompt(
        RenderSkillPromptInput {
            skill_name: &skill.name,
            skill_args: &skill_args,
            skill_content: &skill_content,
            skill_source: Some(skill.source),
            skill_dir: Some(&skill.dir),
        },
        prompt_trigger,
    );
    let serialized_origin =
        serde_json::to_value(&origin).expect("skill activation origin is serializable");
    skill_service.record_model_tool_activation(origin);

    Ok(ExecutableToolResult {
        delivery: Some(ToolDelivery {
            kind: ToolDeliveryKind::Steer,
            message: ToolDeliveryMessage {
                content: vec![ContentPart::Text { text: prompt }],
                tool_calls: Some(Vec::<ToolCall>::new()),
                origin: Some(serialized_origin),
            },
        }),
        ..ExecutableToolResult::success(format!(
            "Skill \"{}\" loaded inline. Follow its instructions.",
            skill.name
        ))
    })
}

fn context_skill_source(source: SkillSource) -> ContextSkillSource {
    match source {
        SkillSource::Project => ContextSkillSource::Project,
        SkillSource::User => ContextSkillSource::User,
        SkillSource::Extra => ContextSkillSource::Extra,
        SkillSource::Builtin => ContextSkillSource::Builtin,
    }
}

// Original: registerTool(SkillTool).
pub fn register_skill_tool() {
    register_tool(
        Arc::new(|accessor| {
            let catalog = accessor.get(SESSION_SKILL_CATALOG_ID)?;
            let skill = accessor.get(AGENT_SKILL_SERVICE_ID)?;
            let session_context = accessor.get(SESSION_CONTEXT_ID)?;
            Ok(Arc::new(SkillTool::new(
                (*catalog).clone(),
                (*skill).clone(),
                (*session_context).clone(),
            )))
        }),
        ToolContributionOptions::default(),
    );
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::{
        _base::{
            di::lifecycle::{Disposable, DisposeResult},
            event::Event,
            utils::abort::AbortController,
        },
        agent::{
            loop_::TurnHandle,
            skill::{
                AgentSkillServiceContract, AgentSkillServiceError, PreparedSkillPrompt,
                SkillActivationInput,
            },
        },
        app::skill_catalog::{
            InMemorySkillCatalog, RegisterSkillOptions, SkillCatalogContract, SkillContribution,
            SkillDefinition, SkillMetadata, SkillSourceResult,
        },
        session::{
            session_context::{SessionContextInput, make_session_context},
            skill_catalog::{
                SessionSkillCatalogContract, SkillCatalogSinkContract, SkillCatalogSinkOptions,
            },
        },
        tool::{ExecutableToolOutput, ToolExecution},
    };

    struct StubSessionCatalog {
        catalog: Arc<dyn SkillCatalogContract>,
        ready_calls: AtomicUsize,
    }

    impl SkillCatalogSinkContract for StubSessionCatalog {
        fn set(
            &self,
            _id: &str,
            _contribution: SkillContribution,
            _options: SkillCatalogSinkOptions,
        ) {
        }

        fn remove(&self, _id: &str) {}
    }

    #[async_trait]
    impl SessionSkillCatalogContract for StubSessionCatalog {
        fn catalog(&self) -> Arc<dyn SkillCatalogContract> {
            Arc::clone(&self.catalog)
        }

        fn on_did_change(&self) -> Event<String> {
            Event::none()
        }

        async fn ready(&self) -> SkillSourceResult<()> {
            self.ready_calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        async fn load(&self) -> SkillSourceResult<()> {
            Ok(())
        }

        async fn reload(&self) -> SkillSourceResult<()> {
            Ok(())
        }
    }

    impl Disposable for StubSessionCatalog {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    #[derive(Default)]
    struct StubSkillService {
        origins: Mutex<Vec<SkillActivationOrigin>>,
    }

    #[async_trait]
    impl AgentSkillServiceContract for StubSkillService {
        async fn activate(
            &self,
            _input: SkillActivationInput,
        ) -> Result<TurnHandle, AgentSkillServiceError> {
            Err(Box::new(std::io::Error::other("not implemented")))
        }

        async fn prepare_prompt_skills(
            &self,
            _inputs: Vec<SkillActivationInput>,
            _shared_args: Option<String>,
        ) -> Result<PreparedSkillPrompt, AgentSkillServiceError> {
            Err(Box::new(std::io::Error::other("not implemented")))
        }

        fn record_user_activations(&self, origins: &[SkillActivationOrigin]) {
            self.origins.lock().extend_from_slice(origins);
        }

        fn record_model_tool_activation(&self, origin: SkillActivationOrigin) {
            self.origins.lock().push(origin);
        }
    }

    impl Disposable for StubSkillService {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    fn skill(name: &str, kind: Option<&str>, disable_model_invocation: bool) -> SkillDefinition {
        SkillDefinition {
            name: name.into(),
            description: format!("{name} changes"),
            path: format!("/skills/{name}/SKILL.md"),
            dir: format!("/skills/{name}"),
            content: "# Commit".into(),
            metadata: SkillMetadata {
                kind: kind.map(str::to_owned),
                disable_model_invocation: disable_model_invocation.then_some(true),
                ..SkillMetadata::default()
            },
            source: SkillSource::User,
            plugin: None,
            mermaid: None,
            d2: None,
        }
    }

    fn setup(
        skills: impl IntoIterator<Item = SkillDefinition>,
    ) -> (
        SkillTool,
        SessionSkillCatalogHandle,
        Arc<StubSessionCatalog>,
        AgentSkillServiceHandle,
        Arc<StubSkillService>,
    ) {
        let mut catalog = InMemorySkillCatalog::default();
        for skill in skills {
            catalog.register(skill, RegisterSkillOptions::default());
        }
        let session_catalog = Arc::new(StubSessionCatalog {
            catalog: Arc::new(catalog),
            ready_calls: AtomicUsize::new(0),
        });
        let catalog_handle = SessionSkillCatalogHandle(session_catalog.clone());
        let skill_service = Arc::new(StubSkillService::default());
        let skill_handle = AgentSkillServiceHandle(skill_service.clone());
        let context = make_session_context(SessionContextInput {
            session_id: "test-session".into(),
            workspace_id: "test-workspace".into(),
            session_dir: "/sessions/test".into(),
            session_scope: "sessions/test".into(),
            cwd: "/workspace".into(),
            meta_scope: None,
        });
        (
            SkillTool::new(catalog_handle.clone(), skill_handle.clone(), context),
            catalog_handle,
            session_catalog,
            skill_handle,
            skill_service,
        )
    }

    fn execution_context() -> ExecutableToolContext {
        ExecutableToolContext {
            turn_id: crate::agent::TurnId::new(0),
            tool_call_id: "call_skill".into(),
            trace: None,
            metadata: None,
            signal: AbortController::new().signal(),
            on_update: None,
            on_foreground_task_start: None,
        }
    }

    fn text_output(result: &ExecutableToolResult) -> &str {
        let ExecutableToolOutput::Text(output) = &result.output else {
            panic!("Skill returns text output")
        };
        output
    }

    #[test]
    fn metadata_schema_and_input_validation_match_the_source_tool() {
        let (tool, ..) = setup([]);
        assert_eq!(tool.tool().name, "Skill");
        assert!(
            tool.tool()
                .description
                .contains("Invoke a registered skill")
        );
        assert!(tool.tool().description.contains("kimi-skill-loaded"));
        assert!(tool.tool().description.contains("with the same `args`"));
        assert!(!tool.tool().description.ends_with('\n'));
        assert_eq!(tool.tool().parameters["type"], "object");
        assert_eq!(tool.tool().parameters["required"], json!(["skill"]));
        assert_eq!(tool.tool().parameters["additionalProperties"], false);
        assert_eq!(
            tool.tool().parameters["properties"]["skill"]["type"],
            "string"
        );
        assert_eq!(
            tool.tool().parameters["properties"]["args"]["type"],
            "string"
        );

        assert!(parse_skill_tool_input(&json!({"skill": "commit"})).is_ok());
        assert!(parse_skill_tool_input(&json!({"skill": "commit", "args": "-m fix"})).is_ok());
        assert!(parse_skill_tool_input(&json!({})).is_err());
        assert!(parse_skill_tool_input(&json!({"skill": "commit", "args": null})).is_err());
        assert!(parse_skill_tool_input(&json!({"skill": "commit", "extra": true})).is_err());
    }

    #[tokio::test]
    async fn resolve_execution_exposes_display_approval_and_glob_matching() {
        let (tool, ..) = setup([]);
        let ToolExecution::Runnable(execution) = tool
            .resolve_execution(SkillToolInput {
                skill: "commit".into(),
                args: Some("-m fix".into()),
            })
            .await
        else {
            panic!("Skill must resolve to a runnable execution")
        };
        assert_eq!(
            execution.description.as_deref(),
            Some("Invoke skill commit")
        );
        assert_eq!(execution.approval_rule, "Skill");
        assert_eq!(
            execution.display,
            Some(ToolInputDisplay::SkillCall {
                skill_name: "commit".into(),
                args: Some("-m fix".into()),
            })
        );
        assert!(execution.matches_rule("commit"));
        assert!(execution.matches_rule("com*"));
        assert!(!execution.matches_rule("review"));
    }

    #[tokio::test]
    async fn rejects_unknown_disabled_and_non_inline_skills_with_exact_messages() {
        let (tool, ..) = setup([
            skill("private", None, true),
            skill("flow-only", Some("flow"), false),
        ]);
        let cases = [
            (
                "missing",
                "Skill \"missing\" not found in the current skill listing.",
            ),
            (
                "private",
                "Skill \"private\" can only be triggered by the user (model invocation is disabled).",
            ),
            (
                "flow-only",
                "Skill \"flow-only\" is not an inline skill and cannot be invoked by the model in v1.",
            ),
        ];
        for (name, expected) in cases {
            let ToolExecution::Runnable(execution) = tool
                .resolve_execution(SkillToolInput {
                    skill: name.into(),
                    args: None,
                })
                .await
            else {
                panic!("Skill must be runnable")
            };
            let result = execution.execute(execution_context()).await;
            assert!(result.is_error);
            assert_eq!(text_output(&result), expected);
        }
    }

    #[tokio::test]
    async fn loads_inline_skill_via_steer_without_exposing_its_body_in_output() {
        let (tool, _, session_catalog, _, skill_service) = setup([skill("commit", None, false)]);
        let ToolExecution::Runnable(execution) = tool
            .resolve_execution(SkillToolInput {
                skill: "commit".into(),
                args: Some("src/app.ts".into()),
            })
            .await
        else {
            panic!("Skill must be runnable")
        };
        let result = execution.execute(execution_context()).await;
        assert!(!result.is_error);
        assert_eq!(
            text_output(&result),
            "Skill \"commit\" loaded inline. Follow its instructions."
        );
        assert!(!text_output(&result).contains("# Commit"));
        assert_eq!(session_catalog.ready_calls.load(Ordering::Relaxed), 1);

        let delivery = result
            .delivery
            .expect("inline skill declares steer delivery");
        assert_eq!(delivery.kind, ToolDeliveryKind::Steer);
        assert_eq!(delivery.message.tool_calls, Some(Vec::new()));
        let origin: SkillActivationOrigin =
            serde_json::from_value(delivery.message.origin.unwrap()).unwrap();
        assert_eq!(origin.skill_name, "commit");
        assert_eq!(origin.skill_args.as_deref(), Some("src/app.ts"));
        assert_eq!(origin.trigger, SkillActivationTrigger::ModelTool);
        let ContentPart::Text { text } = &delivery.message.content[0] else {
            panic!("skill delivery is text")
        };
        assert!(text.contains(
            "<kimi-skill-loaded name=\"commit\" trigger=\"model-tool\" source=\"user\" dir=\"/skills/commit\" args=\"src/app.ts\">"
        ));
        assert!(text.contains("ARGUMENTS: src/app.ts"));

        let recorded = skill_service.origins.lock();
        assert_eq!(recorded.as_slice(), &[origin]);
    }

    #[tokio::test]
    async fn initial_depth_selects_nested_trigger_and_enforces_the_limit() {
        let (tool, catalog, _, service, skill_service) = setup([skill("commit", None, false)]);
        let nested = tool.with_initial_query_depth(2);
        let ToolExecution::Runnable(execution) = nested
            .resolve_execution(SkillToolInput {
                skill: "commit".into(),
                args: None,
            })
            .await
        else {
            panic!("Skill must be runnable")
        };
        let result = execution.execute(execution_context()).await;
        let origin: SkillActivationOrigin =
            serde_json::from_value(result.delivery.unwrap().message.origin.unwrap()).unwrap();
        assert_eq!(origin.trigger, SkillActivationTrigger::NestedSkill);
        assert_eq!(origin.skill_args, None);

        let error = execute_model_skill(
            &catalog,
            &service,
            SkillToolInput {
                skill: "commit".into(),
                args: None,
            },
            MAX_SKILL_QUERY_DEPTH,
            "test-session",
        )
        .await
        .unwrap_err();
        let ExecuteModelSkillError::NestedSkillTooDeep(error) = error else {
            panic!("depth violation must retain its structured error")
        };
        assert_eq!(error.depth, MAX_SKILL_QUERY_DEPTH);
        assert_eq!(error.skill_name.as_deref(), Some("commit"));
        assert_eq!(
            error.to_string(),
            "Nested skill invocation \"commit\" exceeded the maximum depth of 3 — refusing to recurse further."
        );
        assert_eq!(skill_service.origins.lock().len(), 1);
    }
}
