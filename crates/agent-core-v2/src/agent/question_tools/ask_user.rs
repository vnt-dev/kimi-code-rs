//! Structured ask-user tool.
//!
//! Original: `agent/questionTools/tools/ask-user.ts`, `AskUserQuestionTool`.

use std::{
    collections::HashSet,
    sync::{Arc, LazyLock},
};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};

use crate::{
    _base::{
        di::instantiation::ServicesAccessorExt,
        utils::abort::{AbortError, AbortSignal},
    },
    agent::{
        question_tools::{QuestionBackgroundTask, QuestionTaskRun},
        scope_context::{AGENT_SCOPE_CONTEXT_ID, AgentScopeContext},
        task::{AGENT_TASK_SERVICE_ID, AgentTaskServiceHandle, RegisterAgentTaskOptions},
        tool_registry::{ToolContributionOptions, register_tool},
    },
    app::telemetry::{
        QuestionAnsweredEvent, QuestionDismissedEvent, TELEMETRY_SERVICE_ID,
        TelemetryServiceHandle,
        event_payloads::{
            QuestionAnswerMethod as TelemetryQuestionAnswerMethod, TelemetryServiceEventExt,
        },
    },
    kosong::contract::{request_trace::LlmRequestTrace, tool::Tool},
    session::question::{
        QuestionAnswerMethod, QuestionItem, QuestionOption, QuestionRequest,
        QuestionRequestOptions, QuestionResult, SESSION_QUESTION_SERVICE_ID,
        SessionQuestionService,
    },
    tool::{
        ExecutableTool, ExecutableToolContext, ExecutableToolResult, RunnableToolExecution,
        ToolExecution, input_schema::to_input_json_schema,
    },
};

const QUESTION_DISMISSED_MESSAGE: &str = "User dismissed the question without answering.";
const QUESTION_UNIQUENESS_MESSAGE: &str = "Question texts must be unique across questions, and option labels must be unique within each question.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AskUserQuestionItem {
    pub question: String,
    pub header: String,
    pub options: Vec<AskUserQuestionOption>,
    pub multi_select: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AskUserQuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AskUserQuestionInput {
    pub background: bool,
    pub questions: Vec<AskUserQuestionItem>,
}

impl<'de> Deserialize<'de> for AskUserQuestionInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        parse_ask_user_question_input(&Value::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

pub fn parse_ask_user_question_input(value: &Value) -> Result<AskUserQuestionInput, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "AskUserQuestion input must be an object".to_owned())?;
    let background = match object.get("background") {
        None => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err("background must be a boolean".into()),
    };
    let questions = object
        .get("questions")
        .and_then(Value::as_array)
        .ok_or_else(|| "questions must be an array".to_owned())?;
    if !(1..=4).contains(&questions.len()) {
        return Err("questions must contain 1 to 4 questions".into());
    }
    let questions = questions
        .iter()
        .map(parse_question_item)
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(error) = question_uniqueness_error(&questions) {
        return Err(error);
    }
    Ok(AskUserQuestionInput {
        background,
        questions,
    })
}

fn parse_question_item(value: &Value) -> Result<AskUserQuestionItem, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "each question must be an object".to_owned())?;
    let question = required_nonempty_string(object, "question")?;
    let header = optional_string(object, "header")?.unwrap_or_default();
    let multi_select = match object.get("multi_select") {
        None => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err("multi_select must be a boolean".into()),
    };
    let options = object
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| "options must be an array".to_owned())?;
    if !(2..=4).contains(&options.len()) {
        return Err("options must contain 2 to 4 options".into());
    }
    let options = options
        .iter()
        .map(|value| {
            let object = value
                .as_object()
                .ok_or_else(|| "each option must be an object".to_owned())?;
            Ok(AskUserQuestionOption {
                label: required_nonempty_string(object, "label")?,
                description: optional_string(object, "description")?.unwrap_or_default(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(AskUserQuestionItem {
        question,
        header,
        options,
        multi_select,
    })
}

fn required_nonempty_string(object: &Map<String, Value>, key: &str) -> Result<String, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("{key} must be a non-empty string"))
}
fn optional_string(object: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

pub fn question_uniqueness_error(questions: &[AskUserQuestionItem]) -> Option<String> {
    let mut texts = HashSet::new();
    for question in questions {
        if !texts.insert(&question.question) {
            return Some(format!(
                "Invalid questions: duplicate question text {:?}. {QUESTION_UNIQUENESS_MESSAGE} Rephrase the duplicates and call the tool again.",
                question.question
            ));
        }
        let mut labels = HashSet::new();
        for option in &question.options {
            if !labels.insert(&option.label) {
                return Some(format!(
                    "Invalid questions: duplicate option label {:?} in question {:?}. {QUESTION_UNIQUENESS_MESSAGE} Rephrase the duplicates and call the tool again.",
                    option.label, question.question
                ));
            }
        }
    }
    None
}

pub static ASK_USER_QUESTION_PARAMETERS: LazyLock<Map<String, Value>> = LazyLock::new(|| {
    to_input_json_schema(json!({
    "type":"object", "properties": {
        "background":{"type":"boolean","default":false},
        "questions":{"type":"array","minItems":1,"maxItems":4,"items":{"type":"object","properties":{
            "question":{"type":"string","minLength":1}, "header":{"type":"string","default":""},
            "options":{"type":"array","minItems":2,"maxItems":4,"items":{"type":"object","properties":{"label":{"type":"string","minLength":1},"description":{"type":"string","default":""}},"required":["label"]}},
            "multi_select":{"type":"boolean","default":false}
        },"required":["question","options"]}}
    }, "required":["questions"]
}).as_object().cloned().expect("ask-user schema is an object"))
});

#[derive(Clone)]
struct QuestionExecutionServices {
    question: Arc<SessionQuestionService>,
    telemetry: TelemetryServiceHandle,
    agent_id: String,
}

pub struct AskUserQuestionTool {
    services: QuestionExecutionServices,
    tasks: AgentTaskServiceHandle,
    definition: Tool,
}

impl AskUserQuestionTool {
    pub fn new(
        question: Arc<SessionQuestionService>,
        telemetry: TelemetryServiceHandle,
        tasks: AgentTaskServiceHandle,
        scope: AgentScopeContext,
    ) -> Self {
        Self {
            services: QuestionExecutionServices {
                question,
                telemetry,
                agent_id: scope.agent_id,
            },
            tasks,
            definition: Tool {
                name: "AskUserQuestion".into(),
                description: include_str!("ask-user.md").into(),
                parameters: ASK_USER_QUESTION_PARAMETERS.clone(),
                deferred: None,
            },
        }
    }
}

// Original: registerTool(AskUserQuestionTool).
pub fn register_ask_user_question_tool() {
    register_tool(
        Arc::new(|accessor| {
            let questions = accessor.get(SESSION_QUESTION_SERVICE_ID)?;
            let telemetry = accessor.get(TELEMETRY_SERVICE_ID)?;
            let tasks = accessor.get(AGENT_TASK_SERVICE_ID)?;
            let scope = accessor.get(AGENT_SCOPE_CONTEXT_ID)?;
            Ok(Arc::new(AskUserQuestionTool::new(
                Arc::clone(&questions.0),
                (*telemetry).clone(),
                (*tasks).clone(),
                (*scope).clone(),
            )))
        }),
        ToolContributionOptions::default(),
    );
}

#[async_trait]
impl ExecutableTool for AskUserQuestionTool {
    type Input = AskUserQuestionInput;
    fn tool(&self) -> &Tool {
        &self.definition
    }
    async fn resolve_execution(&self, args: AskUserQuestionInput) -> ToolExecution {
        let description = if args.background {
            format!(
                "Starting background question: {}",
                question_description(&args.questions)
            )
        } else {
            "Asking user questions".into()
        };
        let services = self.services.clone();
        let tasks = self.tasks.clone();
        let execute = Arc::new(move |context: ExecutableToolContext| {
            let args = args.clone();
            let services = services.clone();
            let tasks = tasks.clone();
            Box::pin(async move {
                if args.background {
                    execute_in_background(&tasks, services, args, context).await
                } else {
                    execute_question(
                        services,
                        args,
                        context.turn_id,
                        context.tool_call_id,
                        context.signal,
                        context.trace,
                    )
                    .await
                    .unwrap_or_else(|error| ExecutableToolResult::error(error.to_string()))
                }
            }) as BoxFuture<'static, ExecutableToolResult>
        });
        let mut execution = RunnableToolExecution::new("AskUserQuestion", execute);
        execution.description = Some(description);
        ToolExecution::Runnable(execution)
    }
}

async fn execute_in_background(
    tasks: &AgentTaskServiceHandle,
    services: QuestionExecutionServices,
    args: AskUserQuestionInput,
    context: ExecutableToolContext,
) -> ExecutableToolResult {
    if let Err(error) = context.signal.throw_if_aborted() {
        return ExecutableToolResult::error(error.to_string());
    }
    let description = question_description(&args.questions);
    let question_count = args.questions.len() as u64;
    let run_args = args.clone();
    let tool_call_id = context.tool_call_id.clone();
    let turn_id = context.turn_id;
    let trace = context.trace.clone();
    let run: QuestionTaskRun = Arc::new(move |signal| {
        let services = services.clone();
        let args = run_args.clone();
        let tool_call_id = tool_call_id.clone();
        let trace = trace.clone();
        Box::pin(async move {
            execute_question(services, args, turn_id, tool_call_id, signal, trace).await
        })
    });
    let task_id = match tasks.register_task(
        Arc::new(QuestionBackgroundTask::new(
            run,
            description.clone(),
            question_count,
            Some(context.tool_call_id),
        )),
        RegisterAgentTaskOptions {
            detached: Some(true),
            ..Default::default()
        },
    ) {
        Ok(id) => id,
        Err(error) => return ExecutableToolResult::error(error.to_string()),
    };
    let status = tasks
        .get_task(&task_id)
        .map_or("running", |info| match info.base.status {
            crate::agent::task::AgentTaskStatus::Running => "running",
            _ => "running",
        });
    ExecutableToolResult::success(format!(
        "task_id: {task_id}\ndescription: {description}\nstatus: {status}\nautomatic_notification: true\nnext_step: Continue your current work; the answer will arrive automatically when the user responds.\nnext_step: Use TaskOutput with this task_id for a non-blocking status/answer snapshot.\nnext_step: Use TaskStop only if the question should be cancelled.\nhuman_shell_hint: The pending question is also visible in /tasks."
    ))
}

async fn execute_question(
    services: QuestionExecutionServices,
    args: AskUserQuestionInput,
    turn_id: crate::agent::TurnId,
    tool_call_id: String,
    signal: AbortSignal,
    trace: Option<LlmRequestTrace>,
) -> Result<ExecutableToolResult, crate::agent::task::AgentTaskError> {
    let result = services
        .question
        .request(
            QuestionRequest {
                id: None,
                turn_id: Some(turn_id),
                tool_call_id: Some(tool_call_id),
                presentation: None,
                questions: args
                    .questions
                    .into_iter()
                    .map(|question| QuestionItem {
                        question: question.question,
                        header: Some(question.header),
                        body: None,
                        options: question
                            .options
                            .into_iter()
                            .map(|option| QuestionOption {
                                label: option.label,
                                description: Some(option.description),
                            })
                            .collect(),
                        multi_select: Some(question.multi_select),
                        other_label: None,
                        other_description: None,
                    })
                    .collect(),
            },
            Some(QuestionRequestOptions {
                signal: Some(signal.clone()),
                agent_id: Some(services.agent_id.clone()),
            }),
        )
        .await;
    if signal.aborted() {
        return Err(Box::new(
            signal
                .reason()
                .map(|reason| (*reason).clone())
                .unwrap_or_else(|| AbortError::new("Aborted")),
        ));
    }
    let Some((answers, method)) = normalize_question_result(result) else {
        let _ = services.telemetry.track_event(&QuestionDismissedEvent {
            trace_id: trace.and_then(|trace| trace.trace_id()),
        });
        return Ok(dismissed_question_result());
    };
    if answers.is_empty() {
        let _ = services.telemetry.track_event(&QuestionDismissedEvent {
            trace_id: trace.and_then(|trace| trace.trace_id()),
        });
        return Ok(dismissed_question_result());
    }
    let _ = services.telemetry.track_event(&QuestionAnsweredEvent {
        answered: answers.len() as u64,
        method: method.map(telemetry_method),
        trace_id: trace.and_then(|trace| trace.trace_id()),
    });
    Ok(ExecutableToolResult::success(
        json!({"answers": answers}).to_string(),
    ))
}

fn normalize_question_result(
    result: Option<QuestionResult>,
) -> Option<(
    crate::session::question::QuestionAnswers,
    Option<QuestionAnswerMethod>,
)> {
    match result {
        None => None,
        Some(QuestionResult::Answers(answers)) => Some((answers, None)),
        Some(QuestionResult::Response(response)) => Some((response.answers, response.method)),
    }
}
fn telemetry_method(method: QuestionAnswerMethod) -> TelemetryQuestionAnswerMethod {
    match method {
        QuestionAnswerMethod::Enter => TelemetryQuestionAnswerMethod::Enter,
        QuestionAnswerMethod::Space => TelemetryQuestionAnswerMethod::Space,
        QuestionAnswerMethod::NumberKey => TelemetryQuestionAnswerMethod::NumberKey,
    }
}
fn dismissed_question_result() -> ExecutableToolResult {
    ExecutableToolResult::success(
        json!({"answers":{},"note":QUESTION_DISMISSED_MESSAGE}).to_string(),
    )
}
pub fn question_description(questions: &[AskUserQuestionItem]) -> String {
    let first = questions
        .first()
        .map(|question| question.question.trim())
        .filter(|question| !question.is_empty())
        .unwrap_or("Ask user question");
    if questions.len() <= 1 {
        first.into()
    } else {
        format!("{first} (+{} more)", questions.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_cardinality_types_and_uniqueness() {
        assert!(parse_ask_user_question_input(&json!({"questions":[{"question":"Pick?","options":[{"label":"Yes"},{"label":"No"}]}]})).is_ok());
        assert!(parse_ask_user_question_input(&json!({"questions":[{"question":"Pick?","options":[{"label":"Yes"},{"label":"Yes"}]}]})).unwrap_err().contains("duplicate option label"));
        assert!(parse_ask_user_question_input(&json!({"questions":[]})).is_err());
    }
    #[test]
    fn describes_first_question_and_remaining_count() {
        let item = |question: &str| AskUserQuestionItem {
            question: question.into(),
            header: String::new(),
            options: vec![],
            multi_select: false,
        };
        assert_eq!(question_description(&[item(" Pick? ")]), "Pick?");
        assert_eq!(
            question_description(&[item("Pick?"), item("Why?")]),
            "Pick? (+1 more)"
        );
    }
}
