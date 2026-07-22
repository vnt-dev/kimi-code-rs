use serde::{Deserialize, Serialize};

use crate::validation::{literal_false, literal_true};
use crate::{IsoDateTime, QuestionRequest, QuestionResponse};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PendingQuestionStatus {
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListPendingQuestionsQuery {
    pub status: PendingQuestionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListPendingQuestionsResponse {
    pub items: Vec<QuestionRequest>,
}

pub type QuestionResolveRequest = QuestionResponse;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionResolveResult {
    #[serde(deserialize_with = "literal_true")]
    pub resolved: bool,
    pub resolved_at: IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAlreadyResolvedData {
    #[serde(deserialize_with = "literal_false")]
    pub resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionDismissResult {
    #[serde(deserialize_with = "literal_true")]
    pub dismissed: bool,
    pub dismissed_at: IsoDateTime,
}
