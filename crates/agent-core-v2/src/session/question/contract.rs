//! Ask-user request and response contracts.
//!
//! Original: `session/question/question.ts`.

use std::{collections::HashMap, ops::Deref, sync::Arc};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::_base::di::instantiation::ServiceIdentifier;

use super::service::SessionQuestionService;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionItem {
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub options: Vec<QuestionOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi_select: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other_description: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum QuestionAnswerMethod {
    #[serde(rename = "enter")]
    Enter,
    #[serde(rename = "space")]
    Space,
    #[serde(rename = "number_key")]
    NumberKey,
}

/// A question answer is either a selected option label or the `true` marker
/// used by the multi-select wire shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuestionAnswer {
    Text(String),
    Selected,
}

impl Serialize for QuestionAnswer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Text(value) => serializer.serialize_str(value),
            Self::Selected => serializer.serialize_bool(true),
        }
    }
}

impl<'de> Deserialize<'de> for QuestionAnswer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::String(value) => Ok(Self::Text(value)),
            serde_json::Value::Bool(true) => Ok(Self::Selected),
            _ => Err(D::Error::custom("question answers must be strings or true")),
        }
    }
}

pub type QuestionAnswers = HashMap<String, QuestionAnswer>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionResponse {
    pub answers: QuestionAnswers,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<QuestionAnswerMethod>,
}

/// The source accepts either the legacy answer map or the richer response
/// object. `None` represents a dismissed question.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum QuestionResult {
    Answers(QuestionAnswers),
    Response(QuestionResponse),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionPresentation {
    RetryConfirmation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<QuestionPresentation>,
    pub questions: Vec<QuestionItem>,
}

#[derive(Clone)]
pub struct SessionQuestionServiceHandle(pub Arc<SessionQuestionService>);

impl Deref for SessionQuestionServiceHandle {
    type Target = SessionQuestionService;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_QUESTION_SERVICE_ID: ServiceIdentifier<SessionQuestionServiceHandle> =
    ServiceIdentifier::new("sessionQuestionService");

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn service_identifier_matches_the_source_decorator() {
        assert_eq!(
            SESSION_QUESTION_SERVICE_ID.to_string(),
            "sessionQuestionService"
        );
    }

    #[test]
    fn retry_confirmation_presentation_uses_its_wire_discriminator() {
        let request = QuestionRequest {
            id: Some("retry-1".into()),
            turn_id: Some(7),
            tool_call_id: None,
            presentation: Some(QuestionPresentation::RetryConfirmation),
            questions: Vec::new(),
        };

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({
                "id": "retry-1",
                "turnId": 7,
                "presentation": "retry_confirmation",
                "questions": []
            })
        );
    }
}
