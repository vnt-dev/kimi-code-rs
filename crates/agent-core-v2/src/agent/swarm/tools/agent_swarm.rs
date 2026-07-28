//! The model-facing AgentSwarm collaboration tool.
//!
//! Original: `packages/agent-core-v2/src/agent/swarm/tools/agent-swarm.ts`.

use std::{io, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    _base::{di::instantiation::ServicesAccessorExt, lifecycle::lifecycle_machine::BoxError},
    agent::{
        profile::{AGENT_PROFILE_SERVICE_ID, AgentProfileServiceHandle},
        scope_context::{AGENT_SCOPE_CONTEXT_ID, AgentScopeContext},
        swarm::{AGENT_SWARM_SERVICE_ID, AgentSwarmServiceHandle, SwarmModeTrigger},
        tool_registry::{ToolContributionOptions, register_tool},
    },
    app::{
        agent_profile_catalog::{subagent_allowlist_for, subagent_type_not_allowed_message},
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
    },
    kosong::contract::tool::Tool,
    session::{
        agent_profile_catalog::{
            SESSION_AGENT_PROFILE_CATALOG_ID, SessionAgentProfileCatalogHandle,
        },
        subagent::resolve_subagent_timeout_ms,
        swarm::{
            SESSION_SWARM_SERVICE_ID, SessionSwarmRunArgs, SessionSwarmRunState,
            SessionSwarmRunStatus, SessionSwarmServiceHandle, SessionSwarmTask,
            SessionSwarmTaskBase,
        },
    },
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolAccess, ToolExecution, ToolInputDisplay, input_schema::to_input_json_schema,
    },
};

use super::super::{
    AgentSwarmInput, AgentSwarmResult, AgentSwarmSpec, AgentSwarmState, AgentSwarmStatus,
    DEFAULT_SUBAGENT_TYPE, MAX_AGENT_SWARM_SUBAGENTS, child_description, create_agent_swarm_specs,
    render_swarm_results,
};

const AGENT_SWARM_DESCRIPTION: &str = include_str!("agent-swarm.md");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentSwarmToolInput {
    pub description: String,
    pub subagent_type: Option<String>,
    pub prompt_template: Option<String>,
    pub items: Vec<String>,
    /// Ordered pairs preserve the model-provided object order.
    pub resume_agent_ids: Vec<(String, String)>,
}

impl<'de> Deserialize<'de> for AgentSwarmToolInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        parse_agent_swarm_tool_input(&value).map_err(serde::de::Error::custom)
    }
}

pub fn parse_agent_swarm_tool_input(value: &Value) -> Result<AgentSwarmToolInput, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "AgentSwarm input must be an object".to_owned())?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "description" | "subagent_type" | "prompt_template" | "items" | "resume_agent_ids"
        )
    }) {
        return Err("AgentSwarm input contains unknown properties".into());
    }

    let description = required_non_empty_string(object, "description")?;
    let subagent_type = optional_non_empty_string(object, "subagent_type")?;
    let prompt_template = optional_non_empty_string(object, "prompt_template")?;
    let items = match object.get("items") {
        None => Vec::new(),
        Some(Value::Array(items)) if items.len() <= MAX_AGENT_SWARM_SUBAGENTS => items
            .iter()
            .map(|item| {
                item.as_str()
                    .ok_or_else(|| "items must contain only strings".to_owned())
                    .and_then(|item| {
                        non_empty_trimmed(item, "items must not contain empty strings")
                    })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(Value::Array(_)) => {
            return Err(format!(
                "items must contain at most {MAX_AGENT_SWARM_SUBAGENTS} entries"
            ));
        }
        Some(_) => return Err("items must be an array".into()),
    };
    let resume_agent_ids = match object.get("resume_agent_ids") {
        None => Vec::new(),
        Some(Value::Object(entries)) => entries
            .iter()
            .map(|(agent_id, prompt)| {
                let agent_id =
                    non_empty_trimmed(agent_id, "resume_agent_ids keys must not be empty")?;
                let prompt = prompt
                    .as_str()
                    .ok_or_else(|| "resume_agent_ids values must be strings".to_owned())
                    .and_then(|prompt| {
                        non_empty_trimmed(prompt, "resume_agent_ids values must not be empty")
                    })?;
                Ok((agent_id, prompt))
            })
            .collect::<Result<Vec<_>, String>>()?,
        Some(_) => return Err("resume_agent_ids must be an object".into()),
    };

    Ok(AgentSwarmToolInput {
        description,
        subagent_type,
        prompt_template,
        items,
        resume_agent_ids,
    })
}

fn required_non_empty_string(object: &Map<String, Value>, name: &str) -> Result<String, String> {
    object
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{name} must be a string"))
        .and_then(|value| non_empty_trimmed(value, &format!("{name} must not be empty")))
}

fn optional_non_empty_string(
    object: &Map<String, Value>,
    name: &str,
) -> Result<Option<String>, String> {
    match object.get(name) {
        None => Ok(None),
        Some(Value::String(value)) => {
            non_empty_trimmed(value, &format!("{name} must not be empty")).map(Some)
        }
        Some(_) => Err(format!("{name} must be a string")),
    }
}

fn non_empty_trimmed(value: &str, error: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        Err(error.into())
    } else {
        Ok(value.into())
    }
}

pub fn agent_swarm_parameters() -> Map<String, Value> {
    to_input_json_schema(
        json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Short description for the whole swarm."
                },
                "subagent_type": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Subagent type used for every new subagent spawned from items; defaults to coder when omitted. Resumed subagents always keep their original type, so passing subagent_type together with resume_agent_ids is allowed — it only affects the item-based spawns."
                },
                "prompt_template": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Prompt template for each subagent. The {{item}} placeholder is replaced with each item value."
                },
                "items": {
                    "type": "array",
                    "maxItems": MAX_AGENT_SWARM_SUBAGENTS,
                    "items": {
                        "type": "string",
                        "minLength": 1
                    },
                    "description": "Values used to fill {{item}}. Each item launches one new subagent."
                },
                "resume_agent_ids": {
                    "type": "object",
                    "propertyNames": {
                        "type": "string",
                        "minLength": 1
                    },
                    "additionalProperties": {
                        "type": "string",
                        "minLength": 1
                    },
                    "description": "Map of existing subagent agent_id to the prompt used to resume that subagent. These resumed subagents are launched before new item-based subagents."
                }
            },
            "required": ["description"],
            "additionalProperties": false
        })
        .as_object()
        .cloned()
        .expect("AgentSwarm schema is an object"),
    )
}

#[async_trait]
pub trait AgentSwarmToolServices: Send + Sync {
    fn enter_tool_mode(&self) -> Result<(), BoxError>;
    async fn ensure_profile_allowed(&self, profile_name: &str) -> Result<(), BoxError>;
    fn timeout(&self) -> Duration;
    async fn get_swarm_item(
        &self,
        caller_agent_id: &str,
        agent_id: &str,
    ) -> Result<Option<String>, BoxError>;
    async fn run(
        &self,
        args: SessionSwarmRunArgs<Value>,
    ) -> Result<Vec<crate::session::swarm::SessionSwarmRunResult<Value>>, BoxError>;
}

struct DefaultAgentSwarmToolServices {
    swarm_service: SessionSwarmServiceHandle,
    swarm_mode: AgentSwarmServiceHandle,
    config: ConfigServiceHandle,
    catalog: SessionAgentProfileCatalogHandle,
    profile: AgentProfileServiceHandle,
}

#[async_trait]
impl AgentSwarmToolServices for DefaultAgentSwarmToolServices {
    fn enter_tool_mode(&self) -> Result<(), BoxError> {
        self.swarm_mode
            .enter(SwarmModeTrigger::Tool)
            .map_err(|error| Box::new(error) as BoxError)
    }

    async fn ensure_profile_allowed(&self, profile_name: &str) -> Result<(), BoxError> {
        self.catalog.ready().await?;
        let own = self.profile.data()?;
        let default_profile = self.catalog.get_default()?;
        let allowlist = subagent_allowlist_for(
            default_profile.subagents.as_deref(),
            own.config.profile_name.as_deref(),
            own.subagents.as_deref(),
        );
        if let Some(allowlist) = allowlist
            && !allowlist.iter().any(|allowed| allowed == profile_name)
        {
            return Err(other_error(subagent_type_not_allowed_message(
                profile_name,
                allowlist,
            )));
        }
        Ok(())
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(resolve_subagent_timeout_ms(self.config.0.as_ref()))
    }

    async fn get_swarm_item(
        &self,
        caller_agent_id: &str,
        agent_id: &str,
    ) -> Result<Option<String>, BoxError> {
        self.swarm_service
            .get_swarm_item(caller_agent_id, agent_id)
            .await
    }

    async fn run(
        &self,
        args: SessionSwarmRunArgs<Value>,
    ) -> Result<Vec<crate::session::swarm::SessionSwarmRunResult<Value>>, BoxError> {
        self.swarm_service.run(args).await
    }
}

#[derive(Clone)]
pub struct AgentSwarmTool {
    services: Arc<dyn AgentSwarmToolServices>,
    caller_agent_id: String,
    definition: Tool,
}

impl AgentSwarmTool {
    pub fn new(
        swarm_service: SessionSwarmServiceHandle,
        scope_context: AgentScopeContext,
        swarm_mode: AgentSwarmServiceHandle,
        config: ConfigServiceHandle,
        catalog: SessionAgentProfileCatalogHandle,
        profile: AgentProfileServiceHandle,
    ) -> Self {
        Self::with_services(
            scope_context.agent_id,
            Arc::new(DefaultAgentSwarmToolServices {
                swarm_service,
                swarm_mode,
                config,
                catalog,
                profile,
            }),
        )
    }

    pub fn with_services(
        caller_agent_id: impl Into<String>,
        services: Arc<dyn AgentSwarmToolServices>,
    ) -> Self {
        Self {
            services,
            caller_agent_id: caller_agent_id.into(),
            definition: Tool {
                name: "AgentSwarm".into(),
                description: AGENT_SWARM_DESCRIPTION.into(),
                parameters: agent_swarm_parameters(),
                deferred: None,
            },
        }
    }

    async fn execution(
        &self,
        args: AgentSwarmToolInput,
        context: ExecutableToolContext,
    ) -> ExecutableToolResult {
        self.execute_inner(args, &context)
            .await
            .map(ExecutableToolResult::success)
            .unwrap_or_else(|error| ExecutableToolResult::error(error.to_string()))
    }

    async fn execute_inner(
        &self,
        args: AgentSwarmToolInput,
        context: &ExecutableToolContext,
    ) -> Result<String, BoxError> {
        self.services.enter_tool_mode()?;
        self.run_swarm(args, context).await
    }

    async fn run_swarm(
        &self,
        args: AgentSwarmToolInput,
        context: &ExecutableToolContext,
    ) -> Result<String, BoxError> {
        let profile_name = args
            .subagent_type
            .as_deref()
            .and_then(normalize_optional_string)
            .unwrap_or(DEFAULT_SUBAGENT_TYPE)
            .to_owned();
        if !args.items.is_empty() {
            self.services.ensure_profile_allowed(&profile_name).await?;
        }

        let format_input = AgentSwarmInput {
            prompt_template: args.prompt_template,
            items: args.items,
            resume_agent_ids: args.resume_agent_ids,
        };
        let services = Arc::clone(&self.services);
        let caller_agent_id = self.caller_agent_id.clone();
        let specs = create_agent_swarm_specs(&format_input, move |agent_id| {
            let services = Arc::clone(&services);
            let caller_agent_id = caller_agent_id.clone();
            let agent_id = agent_id.to_owned();
            async move {
                services
                    .get_swarm_item(&caller_agent_id, &agent_id)
                    .await
                    .map_err(|error| error.to_string())
            }
        })
        .await
        .map_err(other_error)?;
        let timeout = self.services.timeout();
        let tasks = specs
            .into_iter()
            .map(|spec| {
                let description_name = if matches!(spec, AgentSwarmSpec::Resume { .. }) {
                    "resume"
                } else {
                    &profile_name
                };
                let base = SessionSwarmTaskBase {
                    data: serde_json::to_value(&spec)
                        .expect("AgentSwarmSpec is always serializable"),
                    profile_name: if matches!(spec, AgentSwarmSpec::Resume { .. }) {
                        "subagent".into()
                    } else {
                        profile_name.clone()
                    },
                    parent_tool_call_id: context.tool_call_id.clone(),
                    parent_tool_call_uuid: None,
                    prompt: spec.prompt().to_owned(),
                    description: child_description(
                        &args.description,
                        spec.index(),
                        description_name,
                    ),
                    swarm_index: Some(spec.index() as u64),
                    swarm_item: spec.item().map(str::to_owned),
                    run_in_background: false,
                    timeout: Some(timeout),
                    signal: Some(context.signal.clone()),
                };
                match spec {
                    AgentSwarmSpec::Resume { agent_id, .. } => SessionSwarmTask::Resume {
                        base,
                        resume_agent_id: agent_id,
                    },
                    AgentSwarmSpec::Spawn { .. } => SessionSwarmTask::Spawn(base),
                }
            })
            .collect();
        let results = self
            .services
            .run(SessionSwarmRunArgs {
                caller_agent_id: self.caller_agent_id.clone(),
                tasks,
            })
            .await?;
        let results = results
            .into_iter()
            .map(|result| {
                let spec =
                    serde_json::from_value::<AgentSwarmSpec>(result.task.base().data.clone())
                        .map_err(|error| Box::new(error) as BoxError)?;
                Ok(AgentSwarmResult {
                    spec,
                    agent_id: result.agent_id,
                    status: match result.status {
                        SessionSwarmRunStatus::Completed => AgentSwarmStatus::Completed,
                        SessionSwarmRunStatus::Failed => AgentSwarmStatus::Failed,
                        SessionSwarmRunStatus::Aborted => AgentSwarmStatus::Aborted,
                    },
                    state: result.state.map(|state| match state {
                        SessionSwarmRunState::Started => AgentSwarmState::Started,
                        SessionSwarmRunState::NotStarted => AgentSwarmState::NotStarted,
                    }),
                    result: result.result,
                    error: result.error,
                })
            })
            .collect::<Result<Vec<_>, BoxError>>()?;
        Ok(render_swarm_results(&results))
    }
}

#[async_trait]
impl ExecutableTool for AgentSwarmTool {
    type Input = AgentSwarmToolInput;

    fn tool(&self) -> &Tool {
        &self.definition
    }

    async fn resolve_execution(&self, args: AgentSwarmToolInput) -> ToolExecution {
        let agent_count = args.items.len() + args.resume_agent_ids.len();
        let this = self.clone();
        let execution_args = args.clone();
        let execute = Arc::new(move |context| {
            let this = this.clone();
            let args = execution_args.clone();
            Box::pin(async move { this.execution(args, context).await })
                as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("AgentSwarm", execute);
        execution.accesses = Some(ToolAccess::all());
        execution.description = Some(format!("Launching agent swarm: {}", args.description));
        execution.display = Some(ToolInputDisplay::AgentCall {
            agent_name: format!("swarm ({agent_count} subagents)"),
            prompt: args.description,
            background: None,
        });
        ToolExecution::Runnable(execution)
    }
}

fn normalize_optional_string(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn other_error(error: impl Into<String>) -> BoxError {
    Box::new(io::Error::other(error.into()))
}

// Original: registerTool(AgentSwarmTool).
pub fn register_agent_swarm_tool() {
    register_tool(
        Arc::new(|accessor| {
            Ok(Arc::new(AgentSwarmTool::new(
                (*accessor.get(SESSION_SWARM_SERVICE_ID)?).clone(),
                (*accessor.get(AGENT_SCOPE_CONTEXT_ID)?).clone(),
                (*accessor.get(AGENT_SWARM_SERVICE_ID)?).clone(),
                (*accessor.get(CONFIG_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_AGENT_PROFILE_CATALOG_ID)?).clone(),
                (*accessor.get(AGENT_PROFILE_SERVICE_ID)?).clone(),
            )) as Arc<dyn crate::tool::ErasedExecutableTool>)
        }),
        ToolContributionOptions::default(),
    );
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        session::swarm::{SessionSwarmRunResult, SessionSwarmTask},
        tool::{ExecutableToolOutput, ToolExecution},
    };

    #[derive(Clone)]
    struct StubOutcome {
        agent_id: Option<String>,
        status: SessionSwarmRunStatus,
        state: Option<SessionSwarmRunState>,
        result: Option<String>,
        error: Option<String>,
    }

    struct StubServices {
        enter_calls: AtomicUsize,
        profile_checks: Mutex<Vec<String>>,
        profile_error: Mutex<Option<String>>,
        timeout: Duration,
        items: HashMap<String, String>,
        item_calls: Mutex<Vec<(String, String)>>,
        run_calls: Mutex<Vec<SessionSwarmRunArgs<Value>>>,
        outcomes: Mutex<Vec<StubOutcome>>,
    }

    impl StubServices {
        fn new(timeout: Duration) -> Self {
            Self {
                enter_calls: AtomicUsize::new(0),
                profile_checks: Mutex::new(Vec::new()),
                profile_error: Mutex::new(None),
                timeout,
                items: HashMap::new(),
                item_calls: Mutex::new(Vec::new()),
                run_calls: Mutex::new(Vec::new()),
                outcomes: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl AgentSwarmToolServices for StubServices {
        fn enter_tool_mode(&self) -> Result<(), BoxError> {
            self.enter_calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        async fn ensure_profile_allowed(&self, profile_name: &str) -> Result<(), BoxError> {
            self.profile_checks
                .lock()
                .unwrap()
                .push(profile_name.into());
            if let Some(error) = self.profile_error.lock().unwrap().clone() {
                Err(other_error(error))
            } else {
                Ok(())
            }
        }

        fn timeout(&self) -> Duration {
            self.timeout
        }

        async fn get_swarm_item(
            &self,
            caller_agent_id: &str,
            agent_id: &str,
        ) -> Result<Option<String>, BoxError> {
            self.item_calls
                .lock()
                .unwrap()
                .push((caller_agent_id.into(), agent_id.into()));
            Ok(self.items.get(agent_id).cloned())
        }

        async fn run(
            &self,
            args: SessionSwarmRunArgs<Value>,
        ) -> Result<Vec<SessionSwarmRunResult<Value>>, BoxError> {
            self.run_calls.lock().unwrap().push(args.clone());
            let outcomes = self.outcomes.lock().unwrap().clone();
            Ok(args
                .tasks
                .into_iter()
                .enumerate()
                .map(|(index, task)| {
                    let outcome = outcomes.get(index).cloned().unwrap_or(StubOutcome {
                        agent_id: Some(format!("agent-{}", index + 1)),
                        status: SessionSwarmRunStatus::Completed,
                        state: None,
                        result: Some(format!("result {}", index + 1)),
                        error: None,
                    });
                    SessionSwarmRunResult {
                        task,
                        agent_id: outcome.agent_id,
                        status: outcome.status,
                        state: outcome.state,
                        result: outcome.result,
                        usage: None,
                        error: outcome.error,
                    }
                })
                .collect())
        }
    }

    fn input(items: &[&str]) -> AgentSwarmToolInput {
        AgentSwarmToolInput {
            description: "Review files".into(),
            subagent_type: None,
            prompt_template: Some("Review {{item}}".into()),
            items: items.iter().map(|item| (*item).into()).collect(),
            resume_agent_ids: Vec::new(),
        }
    }

    fn context() -> ExecutableToolContext {
        ExecutableToolContext {
            turn_id: 0,
            tool_call_id: "call_swarm".into(),
            trace: None,
            metadata: None,
            signal: AbortController::new().signal(),
            on_update: None,
            on_foreground_task_start: None,
        }
    }

    fn text_output(result: &ExecutableToolResult) -> &str {
        let ExecutableToolOutput::Text(output) = &result.output else {
            panic!("AgentSwarm returns text")
        };
        output
    }

    #[test]
    fn input_schema_is_strict_trimmed_and_capped_at_128_items() {
        let parsed = parse_agent_swarm_tool_input(&json!({
            "description": " Review files ",
            "subagent_type": " explore ",
            "prompt_template": " Review {{item}} ",
            "items": [" src/a.ts ", "src/b.ts"],
            "resume_agent_ids": {
                " agent-old ": " continue "
            }
        }))
        .unwrap();
        assert_eq!(parsed.description, "Review files");
        assert_eq!(parsed.subagent_type.as_deref(), Some("explore"));
        assert_eq!(parsed.items, ["src/a.ts", "src/b.ts"]);
        assert_eq!(
            parsed.resume_agent_ids,
            [("agent-old".into(), "continue".into())]
        );
        assert!(
            parse_agent_swarm_tool_input(&json!({
                "description": "Review",
                "items": (0..128).map(|index| format!("item-{index}")).collect::<Vec<_>>()
            }))
            .is_ok()
        );
        assert!(
            parse_agent_swarm_tool_input(&json!({
                "description": "Review",
                "items": (0..129).map(|index| format!("item-{index}")).collect::<Vec<_>>()
            }))
            .is_err()
        );
        assert!(parse_agent_swarm_tool_input(&json!({"description": "  "})).is_err());
        assert!(
            parse_agent_swarm_tool_input(&json!({
                "description": "Review",
                "unknown": true
            }))
            .is_err()
        );

        let parameters = agent_swarm_parameters();
        assert_eq!(parameters["required"], json!(["description"]));
        assert_eq!(parameters["additionalProperties"], false);
        assert_eq!(parameters["properties"]["items"]["maxItems"], 128);
        assert_eq!(
            parameters["properties"]
                .as_object()
                .unwrap()
                .keys()
                .next_back()
                .map(String::as_str),
            Some("resume_agent_ids")
        );
        assert!(parameters["properties"].get("run_in_background").is_none());
        assert!(parameters["properties"].get("timeout").is_none());
        assert!(parameters["properties"].get("model").is_none());
    }

    #[tokio::test]
    async fn execution_metadata_declares_exclusive_broad_swarm_call() {
        let services = Arc::new(StubServices::new(Duration::from_secs(1)));
        let tool = AgentSwarmTool::with_services("main", services);
        assert_eq!(tool.tool().name, "AgentSwarm");
        assert!(tool.tool().description.contains("at least 2"));
        assert!(tool.tool().description.contains("{{item}}"));
        assert!(tool.tool().description.contains("128 subagents"));
        assert!(tool.tool().description.contains(
            "If `AgentSwarm` is called, that call must be the only tool call in the response."
        ));

        let mut args = input(&["src/new.ts"]);
        args.description = "Finish review".into();
        args.resume_agent_ids = vec![
            ("agent-old-1".into(), "continue A".into()),
            ("agent-old-2".into(), "continue B".into()),
        ];
        let ToolExecution::Runnable(execution) = tool.resolve_execution(args).await else {
            panic!("AgentSwarm must be runnable")
        };
        assert_eq!(
            execution.description.as_deref(),
            Some("Launching agent swarm: Finish review")
        );
        assert_eq!(execution.accesses, Some(ToolAccess::all()));
        assert_eq!(execution.approval_rule, "AgentSwarm");
        assert!(execution.matches_rule.is_none());
        assert_eq!(
            execution.display,
            Some(ToolInputDisplay::AgentCall {
                agent_name: "swarm (3 subagents)".into(),
                prompt: "Finish review".into(),
                background: None,
            })
        );
    }

    #[tokio::test]
    async fn runs_templated_subagents_with_profile_timeout_and_xml_results() {
        let services = Arc::new(StubServices::new(Duration::from_millis(5_000)));
        let tool = AgentSwarmTool::with_services("main", services.clone());
        let mut args = input(&["src/a.ts", "src/b.ts"]);
        args.subagent_type = Some("explore".into());
        let ToolExecution::Runnable(execution) = tool.resolve_execution(args).await else {
            panic!("AgentSwarm must be runnable")
        };
        let result = execution.execute(context()).await;

        assert!(!result.is_error);
        assert_eq!(services.enter_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            services.profile_checks.lock().unwrap().as_slice(),
            ["explore"]
        );
        let calls = services.run_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].caller_agent_id, "main");
        assert_eq!(calls[0].tasks.len(), 2);
        for (index, task) in calls[0].tasks.iter().enumerate() {
            let base = task.base();
            assert!(matches!(task, SessionSwarmTask::Spawn(_)));
            assert_eq!(base.profile_name, "explore");
            assert_eq!(base.parent_tool_call_id, "call_swarm");
            assert_eq!(base.parent_tool_call_uuid, None);
            assert_eq!(base.swarm_index, Some(index as u64 + 1));
            assert_eq!(base.timeout, Some(Duration::from_millis(5_000)));
            assert!(base.signal.is_some());
            assert!(!base.run_in_background);
            assert_eq!(
                base.data,
                json!({
                    "kind": "spawn",
                    "index": index + 1,
                    "item": if index == 0 { "src/a.ts" } else { "src/b.ts" },
                    "prompt": if index == 0 { "Review src/a.ts" } else { "Review src/b.ts" }
                })
            );
        }
        assert_eq!(
            text_output(&result),
            concat!(
                "<agent_swarm_result>\n",
                "<summary>completed: 2</summary>\n",
                "<subagent agent_id=\"agent-1\" item=\"src/a.ts\" outcome=\"completed\">result 1</subagent>\n",
                "<subagent agent_id=\"agent-2\" item=\"src/b.ts\" outcome=\"completed\">result 2</subagent>\n",
                "</agent_swarm_result>"
            )
        );
    }

    #[tokio::test]
    async fn resumes_agents_before_spawning_and_preserves_persisted_items() {
        let mut services = StubServices::new(Duration::from_secs(2));
        services
            .items
            .insert("agent-old-1".into(), "src/old-a.ts".into());
        services
            .items
            .insert("agent-old-2".into(), "src/old-b.ts".into());
        let services = Arc::new(services);
        let tool = AgentSwarmTool::with_services("main", services.clone());
        let args = AgentSwarmToolInput {
            description: "Finish review".into(),
            subagent_type: Some("explore".into()),
            prompt_template: Some("Review {{item}}".into()),
            items: vec!["src/new.ts".into()],
            resume_agent_ids: vec![
                ("agent-old-1".into(), "Continue A".into()),
                ("agent-old-2".into(), "Continue B".into()),
            ],
        };
        let ToolExecution::Runnable(execution) = tool.resolve_execution(args).await else {
            panic!("AgentSwarm must be runnable")
        };
        let result = execution.execute(context()).await;
        assert!(!result.is_error);
        assert_eq!(
            services.item_calls.lock().unwrap().as_slice(),
            [
                ("main".into(), "agent-old-1".into()),
                ("main".into(), "agent-old-2".into())
            ]
        );
        let calls = services.run_calls.lock().unwrap();
        assert_eq!(calls[0].tasks.len(), 3);
        let SessionSwarmTask::Resume {
            base,
            resume_agent_id,
        } = &calls[0].tasks[0]
        else {
            panic!("resume tasks come first")
        };
        assert_eq!(resume_agent_id, "agent-old-1");
        assert_eq!(base.profile_name, "subagent");
        assert_eq!(base.swarm_item.as_deref(), Some("src/old-a.ts"));
        assert_eq!(base.description, "Finish review #1 (resume)");
        assert_eq!(
            base.data,
            json!({
                "kind": "resume",
                "index": 1,
                "agentId": "agent-old-1",
                "item": "src/old-a.ts",
                "prompt": "Continue A"
            })
        );
        assert!(matches!(calls[0].tasks[2], SessionSwarmTask::Spawn(_)));
        assert_eq!(calls[0].tasks[2].base().profile_name, "explore");
        assert_eq!(calls[0].tasks[2].base().swarm_index, Some(3));
        assert!(text_output(&result).contains(
            "<subagent mode=\"resume\" agent_id=\"agent-1\" item=\"src/old-a.ts\" outcome=\"completed\">result 1</subagent>"
        ));
    }

    #[tokio::test]
    async fn invalid_shapes_and_disallowed_profiles_return_tool_errors_before_run() {
        let services = Arc::new(StubServices::new(Duration::from_secs(1)));
        let tool = AgentSwarmTool::with_services("main", services.clone());
        let cases = [
            (
                input(&["only"]),
                "AgentSwarm requires at least 2 items unless resume_agent_ids is provided.",
            ),
            (
                AgentSwarmToolInput {
                    prompt_template: None,
                    ..input(&["a", "b"])
                },
                "prompt_template is required when items are provided.",
            ),
            (
                AgentSwarmToolInput {
                    prompt_template: Some("Review files".into()),
                    ..input(&["a", "b"])
                },
                "prompt_template must include the {{item}} placeholder.",
            ),
            (
                input(&["same", "same"]),
                "Duplicate subagent prompts from items 1 and 2. AgentSwarm requires distinct subagents.",
            ),
        ];
        for (args, expected) in cases {
            let ToolExecution::Runnable(execution) = tool.resolve_execution(args).await else {
                panic!("AgentSwarm must be runnable")
            };
            let result = execution.execute(context()).await;
            assert!(result.is_error);
            assert_eq!(text_output(&result), expected);
        }
        assert!(services.run_calls.lock().unwrap().is_empty());

        let ToolExecution::Runnable(execution) = tool
            .resolve_execution(AgentSwarmToolInput {
                items: (0..129).map(|index| format!("item-{index}")).collect(),
                ..input(&[])
            })
            .await
        else {
            panic!("AgentSwarm must be runnable")
        };
        let result = execution.execute(context()).await;
        assert!(result.is_error);
        assert_eq!(
            text_output(&result),
            "AgentSwarm supports at most 128 subagents."
        );
        assert!(services.run_calls.lock().unwrap().is_empty());

        *services.profile_error.lock().unwrap() =
            Some("Subagent type \"coder\" is not allowed for this agent.".into());
        let ToolExecution::Runnable(execution) = tool.resolve_execution(input(&["a", "b"])).await
        else {
            panic!("AgentSwarm must be runnable")
        };
        let result = execution.execute(context()).await;
        assert!(result.is_error);
        assert_eq!(
            text_output(&result),
            "Subagent type \"coder\" is not allowed for this agent."
        );
        assert!(services.run_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn maps_failed_and_aborted_children_without_failing_the_tool() {
        let services = Arc::new(StubServices::new(Duration::from_secs(1)));
        *services.outcomes.lock().unwrap() = vec![
            StubOutcome {
                agent_id: Some("agent-1".into()),
                status: SessionSwarmRunStatus::Completed,
                state: None,
                result: Some("done".into()),
                error: None,
            },
            StubOutcome {
                agent_id: Some("agent-2".into()),
                status: SessionSwarmRunStatus::Aborted,
                state: Some(SessionSwarmRunState::Started),
                result: None,
                error: Some("interrupted".into()),
            },
            StubOutcome {
                agent_id: None,
                status: SessionSwarmRunStatus::Failed,
                state: Some(SessionSwarmRunState::NotStarted),
                result: None,
                error: Some("did not start".into()),
            },
        ];
        let tool = AgentSwarmTool::with_services("main", services);
        let ToolExecution::Runnable(execution) =
            tool.resolve_execution(input(&["a", "b", "c"])).await
        else {
            panic!("AgentSwarm must be runnable")
        };
        let result = execution.execute(context()).await;
        assert!(!result.is_error);
        assert_eq!(
            text_output(&result),
            concat!(
                "<agent_swarm_result>\n",
                "<summary>completed: 1, failed: 1, aborted: 1</summary>\n",
                "<resume_hint>Call AgentSwarm with resume_agent_ids using the agent_id values in this result to continue unfinished work.</resume_hint>\n",
                "<subagent agent_id=\"agent-1\" item=\"a\" outcome=\"completed\">done</subagent>\n",
                "<subagent agent_id=\"agent-2\" item=\"b\" state=\"started\" outcome=\"aborted\">interrupted</subagent>\n",
                "<subagent item=\"c\" state=\"not_started\" outcome=\"failed\">did not start</subagent>\n",
                "</agent_swarm_result>"
            )
        );
    }
}
