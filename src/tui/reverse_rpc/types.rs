#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffDisplayBlock {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
    pub old_start: Option<usize>,
    pub new_start: Option<usize>,
    pub is_summary: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileContentDisplayBlock {
    pub path: String,
    pub content: String,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOperation {
    Read,
    Write,
    Edit,
    Glob,
    Grep,
}

impl FileOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Edit => "edit",
            Self::Glob => "glob",
            Self::Grep => "grep",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationKind {
    Agent,
    Skill,
}

impl InvocationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Skill => "skill",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoDisplayStatus {
    Pending,
    InProgress,
    Done,
}

impl TodoDisplayStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Done => "done",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoDisplayItem {
    pub title: String,
    pub status: TodoDisplayStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayBlock {
    Brief {
        text: String,
    },
    Diff(DiffDisplayBlock),
    Shell {
        language: String,
        command: String,
        cwd: Option<String>,
        description: Option<String>,
        danger: Option<String>,
    },
    FileOp {
        operation: FileOperation,
        path: String,
        detail: Option<String>,
    },
    FileContent(FileContentDisplayBlock),
    UrlFetch {
        url: String,
        method: Option<String>,
    },
    Search {
        query: String,
        scope: Option<String>,
    },
    Invocation {
        kind: InvocationKind,
        name: String,
        description: Option<String>,
    },
    Todo {
        items: Vec<TodoDisplayItem>,
    },
    BackgroundTask {
        task_id: String,
        kind: String,
        status: String,
        description: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approved,
    ApprovedForSession,
    Rejected,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPanelChoice {
    pub label: String,
    pub response: ApprovalDecision,
    pub selected_label: Option<String>,
    pub requires_feedback: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPanelData {
    pub id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub action: String,
    pub description: String,
    pub display: Vec<DisplayBlock>,
    pub choices: Vec<ApprovalPanelChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    pub data: ApprovalPanelData,
}

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
