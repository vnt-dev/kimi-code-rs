#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionPanelOption {
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionPanelItem {
    pub question: String,
    pub header: Option<String>,
    pub body: Option<String>,
    pub multi_select: bool,
    pub other_label: Option<String>,
    pub other_description: Option<String>,
    pub options: Vec<QuestionPanelOption>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionPanelData {
    pub id: String,
    pub tool_call_id: String,
    pub questions: Vec<QuestionPanelItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingQuestion {
    pub data: QuestionPanelData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionSubmissionMethod {
    Enter,
    Space,
    NumberKey,
}

/// `None` preserves a hole in the JavaScript `answers` array for an
/// unanswered question before a later answered question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionPanelResponse {
    pub answers: Vec<Option<String>>,
    pub method: Option<QuestionSubmissionMethod>,
}

impl QuestionPanelResponse {
    pub fn cancelled() -> Self {
        Self {
            answers: Vec::new(),
            method: None,
        }
    }
}
