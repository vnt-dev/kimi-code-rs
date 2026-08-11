use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};

use super::time::IsoDateTime;
use super::validation::{non_empty, optional_non_empty};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    #[serde(deserialize_with = "non_empty")]
    pub id: String,
    #[serde(deserialize_with = "non_empty")]
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionItem {
    #[serde(deserialize_with = "non_empty")]
    pub id: String,
    #[serde(deserialize_with = "non_empty")]
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(deserialize_with = "deserialize_options")]
    pub options: Vec<QuestionOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multi_select: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_other: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_description: Option<String>,
}

fn deserialize_options<'de, D>(deserializer: D) -> Result<Vec<QuestionOption>, D::Error>
where
    D: Deserializer<'de>,
{
    let options = Vec::<QuestionOption>::deserialize(deserializer)?;
    if (2..=4).contains(&options.len()) {
        Ok(options)
    } else {
        Err(serde::de::Error::custom(
            "questions require between 2 and 4 options",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionRequest {
    #[serde(deserialize_with = "non_empty")]
    pub question_id: String,
    #[serde(deserialize_with = "non_empty")]
    pub session_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::ids::turn_id::non_negative_option::deserialize"
    )]
    pub turn_id: Option<crate::TurnId>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub tool_call_id: Option<String>,
    #[serde(deserialize_with = "deserialize_questions")]
    pub questions: Vec<QuestionItem>,
    pub created_at: IsoDateTime,
}

fn deserialize_questions<'de, D>(deserializer: D) -> Result<Vec<QuestionItem>, D::Error>
where
    D: Deserializer<'de>,
{
    let questions = Vec::<QuestionItem>::deserialize(deserializer)?;
    if (1..=4).contains(&questions.len()) {
        Ok(questions)
    } else {
        Err(serde::de::Error::custom(
            "request requires between 1 and 4 questions",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuestionAnswer {
    Single {
        #[serde(deserialize_with = "non_empty")]
        option_id: String,
    },
    Multi {
        #[serde(deserialize_with = "deserialize_non_empty_ids")]
        option_ids: Vec<String>,
    },
    Other {
        text: String,
    },
    MultiWithOther {
        #[serde(deserialize_with = "deserialize_ids")]
        option_ids: Vec<String>,
        other_text: String,
    },
    Skipped,
}

fn deserialize_ids<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let ids = Vec::<String>::deserialize(deserializer)?;
    if ids.iter().any(String::is_empty) {
        Err(serde::de::Error::custom("option IDs must not be empty"))
    } else {
        Ok(ids)
    }
}

fn deserialize_non_empty_ids<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let ids = deserialize_ids(deserializer)?;
    if ids.is_empty() {
        Err(serde::de::Error::custom(
            "at least one option ID is required",
        ))
    } else {
        Ok(ids)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionAnswerMethod {
    Enter,
    Space,
    NumberKey,
    Click,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionResponse {
    #[serde(deserialize_with = "deserialize_answers")]
    pub answers: IndexMap<String, QuestionAnswer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<QuestionAnswerMethod>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn deserialize_answers<'de, D>(
    deserializer: D,
) -> Result<IndexMap<String, QuestionAnswer>, D::Error>
where
    D: Deserializer<'de>,
{
    let answers = IndexMap::<String, QuestionAnswer>::deserialize(deserializer)?;
    if answers.keys().any(String::is_empty) {
        Err(serde::de::Error::custom("question IDs must not be empty"))
    } else {
        Ok(answers)
    }
}
