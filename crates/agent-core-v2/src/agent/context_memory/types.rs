use serde::{Deserialize, Serialize};

use crate::{
    agent::task::types::AgentTaskStatus,
    kosong::contract::message::{ContentPart, Message},
};

// Original:
//   packages/agent-core-v2/src/agent/contextMemory/types.ts
//   SkillSource and PromptOrigin
//
// Rust adaptation:
//   The TypeScript discriminated union is an internally tagged enum. Variant
//   fields retain their original camelCase serialized names.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    Project,
    User,
    Extra,
    Builtin,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillActivationTrigger {
    UserSlash,
    ModelTool,
    NestedSkill,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginCommandTrigger {
    UserSlash,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellCommandPhase {
    Input,
    Output,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PromptOrigin {
    User,
    SkillActivation {
        activation_id: String,
        skill_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        skill_args: Option<String>,
        trigger: SkillActivationTrigger,
        #[serde(skip_serializing_if = "Option::is_none")]
        skill_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        skill_path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        skill_source: Option<SkillSource>,
    },
    PluginCommand {
        activation_id: String,
        plugin_id: String,
        command_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        command_args: Option<String>,
        trigger: PluginCommandTrigger,
    },
    Injection {
        variant: String,
    },
    ShellCommand {
        phase: ShellCommandPhase,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    CompactionSummary,
    SystemTrigger {
        name: String,
    },
    Task {
        task_id: String,
        status: AgentTaskStatus,
        notification_id: String,
    },
    CronJob {
        job_id: String,
        cron: String,
        recurring: bool,
        coalesced_count: u64,
        stale: bool,
    },
    CronMissed {
        count: u64,
    },
    HookResult {
        event: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        blocked: Option<bool>,
    },
    Retry {
        #[serde(skip_serializing_if = "Option::is_none")]
        trigger: Option<String>,
    },
}

pub const USER_PROMPT_ORIGIN: PromptOrigin = PromptOrigin::User;

// Original:
//   packages/agent-core-v2/src/agent/contextMemory/types.ts
//   ContextMessage
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextMessage {
    #[serde(flatten)]
    pub message: Message,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<PromptOrigin>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UserMessageRecord {
    pub content: Vec<ContentPart>,
    pub origin: PromptOrigin,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SystemReminderRecord {
    pub content: String,
    pub origin: PromptOrigin,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextData {
    pub history: Vec<ContextMessage>,
    pub token_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::contract::message::{Role, ToolCall};
    use serde_json::json;

    #[test]
    fn prompt_origin_preserves_discriminants_and_field_names() {
        let origin = PromptOrigin::SkillActivation {
            activation_id: "activation-1".into(),
            skill_name: "review".into(),
            skill_args: Some("--strict".into()),
            trigger: SkillActivationTrigger::UserSlash,
            skill_type: None,
            skill_path: None,
            skill_source: Some(SkillSource::Project),
        };

        assert_eq!(
            serde_json::to_value(origin).unwrap(),
            json!({
                "kind": "skill_activation",
                "activationId": "activation-1",
                "skillName": "review",
                "skillArgs": "--strict",
                "trigger": "user-slash",
                "skillSource": "project"
            })
        );
        assert_eq!(
            serde_json::to_value(&USER_PROMPT_ORIGIN).unwrap(),
            json!({ "kind": "user" })
        );
    }

    #[test]
    fn context_message_flattens_kosong_message_shape() {
        let context_message = ContextMessage {
            message: Message::new(
                Role::User,
                vec![ContentPart::Text {
                    text: "hello".into(),
                }],
                Vec::<ToolCall>::new(),
            ),
            id: Some("msg-1".into()),
            provider_message_id: Some("provider-1".into()),
            origin: Some(PromptOrigin::User),
            is_error: None,
            note: None,
        };

        let value = serde_json::to_value(&context_message).unwrap();
        assert_eq!(value["role"], "user");
        assert_eq!(
            value["content"][0],
            json!({ "type": "text", "text": "hello" })
        );
        assert_eq!(value["id"], "msg-1");
        assert_eq!(value["providerMessageId"], "provider-1");
        assert_eq!(value["origin"], json!({ "kind": "user" }));

        assert_eq!(
            serde_json::from_value::<ContextMessage>(value).unwrap(),
            context_message
        );
    }

    #[test]
    fn task_and_cron_origins_preserve_status_and_counts() {
        assert_eq!(
            serde_json::to_value(PromptOrigin::Task {
                task_id: "task-1".into(),
                status: AgentTaskStatus::TimedOut,
                notification_id: "notification-1".into(),
            })
            .unwrap(),
            json!({
                "kind": "task",
                "taskId": "task-1",
                "status": "timed_out",
                "notificationId": "notification-1"
            })
        );
        assert_eq!(
            serde_json::to_value(PromptOrigin::CronMissed { count: 3 }).unwrap(),
            json!({ "kind": "cron_missed", "count": 3 })
        );
    }
}
