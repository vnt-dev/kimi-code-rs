use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum OptionalJsonValue {
    #[default]
    Absent,
    Present(Value),
}

impl OptionalJsonValue {
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }

    pub fn as_value(&self) -> Option<&Value> {
        match self {
            Self::Absent => None,
            Self::Present(value) => Some(value),
        }
    }
}

impl Serialize for OptionalJsonValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Absent => serializer.serialize_unit(),
            Self::Present(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for OptionalJsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Value::deserialize(deserializer).map(Self::Present)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileIoOperation {
    Read,
    Write,
    Edit,
    Glob,
    Grep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalStartMode {
    Manual,
    Yolo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoDisplayItem {
    pub title: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReviewOption {
    pub label: String,
    pub description: String,
}

// Original: display.ts, ToolInputDisplaySchema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolInputDisplay {
    Command {
        command: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        language: Option<CommandLanguage>,
    },
    FileIo {
        operation: FileIoOperation,
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        before: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        after: Option<String>,
    },
    Diff {
        path: String,
        before: String,
        after: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hunks: Option<f64>,
    },
    Search {
        query: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
    },
    UrlFetch {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        method: Option<String>,
    },
    AgentCall {
        agent_name: String,
        prompt: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        background: Option<bool>,
    },
    SkillCall {
        skill_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        args: Option<String>,
    },
    TodoList {
        items: Vec<TodoDisplayItem>,
    },
    Task {
        task_id: String,
        status: String,
        description: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_kind: Option<String>,
    },
    TaskStop {
        task_id: String,
        task_description: String,
    },
    PlanReview {
        plan: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        options: Option<Vec<PlanReviewOption>>,
    },
    GoalStart {
        objective: String,
        #[serde(
            rename = "completionCriterion",
            skip_serializing_if = "Option::is_none"
        )]
        completion_criterion: Option<String>,
        mode: GoalStartMode,
    },
    Generic {
        summary: String,
        #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
        detail: OptionalJsonValue,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandLanguage {
    Bash,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileContentRange {
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResultMatch {
    pub file: String,
    pub line: f64,
    pub text: String,
}

// Original: display.ts, ToolResultDisplaySchema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolResultDisplay {
    CommandOutput {
        exit_code: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        stdout: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stderr: Option<String>,
    },
    FileContent {
        path: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        range: Option<FileContentRange>,
        #[serde(skip_serializing_if = "Option::is_none")]
        truncated: Option<bool>,
    },
    Diff {
        path: String,
        before: String,
        after: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        hunks: Option<f64>,
    },
    SearchResults {
        query: String,
        matches: Vec<SearchResultMatch>,
    },
    UrlContent {
        url: String,
        status: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        preview: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
    AgentSummary {
        agent_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        steps: Option<f64>,
    },
    Task {
        task_id: String,
        status: String,
        description: String,
    },
    TodoList {
        items: Vec<TodoDisplayItem>,
    },
    Structured {
        data: Value,
    },
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        truncated: Option<bool>,
    },
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    Generic {
        summary: String,
        #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
        detail: OptionalJsonValue,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_unions_discriminate_and_preserve_explicit_null_details() {
        let input: ToolInputDisplay = serde_json::from_value(serde_json::json!({
            "kind":"goal_start","objective":"ship","completionCriterion":"tests pass",
            "mode":"manual"
        }))
        .unwrap();
        assert!(matches!(
            input,
            ToolInputDisplay::GoalStart {
                mode: GoalStartMode::Manual,
                ..
            }
        ));

        let result: ToolResultDisplay = serde_json::from_value(serde_json::json!({
            "kind":"generic","summary":"done","detail":null
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::json!({"kind":"generic","summary":"done","detail":null})
        );
        assert!(
            serde_json::from_value::<ToolResultDisplay>(serde_json::json!({
                "kind":"unknown"
            }))
            .is_err()
        );
    }
}
