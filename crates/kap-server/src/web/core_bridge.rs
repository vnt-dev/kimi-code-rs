use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreOperation {
    GetAuth,
    GetConfig,
    UpdateConfig,
    GetOauthLogin,
    StartOauthLogin,
    CancelOauthLogin,
    DeleteOauthLogin,
    OauthLogout,
    ListModels,
    ModelAction,
    ListProviders,
    GetProvider,
    ProviderCollectionAction,
    ProviderAction,
    ListSessions,
    CreateSession,
    GetSession,
    SessionAction,
    SessionNestedAction,
    ListSessionChildren,
    CreateSessionChild,
    GetSessionGoal,
    GetSessionProfile,
    UpdateSessionProfile,
    GetSessionStatus,
    GetSessionWarnings,
    ExportSession,
    ListMessages,
    GetMessage,
    ListPrompts,
    SubmitPrompt,
    SteerPrompt,
    PromptAction,
    ListApprovals,
    ResolveApproval,
    ListQuestions,
    QuestionAction,
    ListSkills,
    SkillAction,
    ListTasks,
    GetTask,
    TaskAction,
    ListTerminals,
    CreateTerminal,
    GetTerminal,
    TerminalAction,
    GetTranscript,
    GetSnapshot,
    ReadSessionFile,
    ListWorkspaces,
    CreateWorkspace,
    UpdateWorkspace,
    DeleteWorkspace,
    ListWorkspaceSkills,
    BrowseFileSystem,
    GetFileSystemHome,
    UploadFile,
    DownloadFile,
    DeleteFile,
    ListTools,
    ListMcpServers,
    McpServerAction,
    DebugChannels,
    DebugGlobalGet,
    DebugGlobalPost,
    DebugSessionGet,
    DebugSessionPost,
    DebugAgentGet,
    DebugAgentPost,
    WebSocketEventReplay,
    WebSocketFileWatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreHttpRequest {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CoreHttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Value,
}

impl CoreHttpResponse {
    pub fn json(body: Value) -> Self {
        Self {
            status: 200,
            headers: BTreeMap::new(),
            body,
        }
    }
}

#[async_trait]
pub trait AgentCoreBridge: Send + Sync {
    async fn invoke(&self, operation: CoreOperation, request: CoreHttpRequest) -> CoreHttpResponse;
}

#[derive(Debug, Default)]
pub struct TodoAgentCoreBridge;

#[async_trait]
impl AgentCoreBridge for TodoAgentCoreBridge {
    async fn invoke(
        &self,
        operation: CoreOperation,
        _request: CoreHttpRequest,
    ) -> CoreHttpResponse {
        // MIGRATION-TODO:
        // Original: packages/kap-server/src/routes/* handlers resolve concrete
        // services from an assembled agent-core-v2 Scope. Bootstrap path
        // resolution, protocol DTOs and time utilities are already used
        // directly; this TODO is limited to service composition/lookup that
        // the Rust bootstrap has not exposed as a complete application Scope.
        todo!("call kimi-code-agent-core-v2 for {operation:?}")
    }
}
