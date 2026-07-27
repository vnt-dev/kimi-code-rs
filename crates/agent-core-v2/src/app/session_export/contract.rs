use crate::_base::di::instantiation::ServiceIdentifier;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{error::Error, ops::Deref, sync::Arc};
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellEnvironment {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub term: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub term_program: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub term_program_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiplexer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSessionPayload {
    pub session_id: String,
    pub output_path: Option<String>,
    pub include_global_log: Option<bool>,
    pub version: String,
    pub install_source: Option<String>,
    pub shell_env: Option<ShellEnvironment>,
}
#[derive(Clone, Debug, Default)]
pub struct ExportSessionOptions {
    pub web_log: Option<String>,
    pub max_archive_bytes: Option<u64>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSessionManifest {
    pub session_id: String,
    pub exported_at: String,
    pub kimi_code_version: String,
    pub wire_protocol_version: String,
    pub os: String,
    pub nodejs_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_first_activity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_last_activity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_log_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_log_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_log_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell_env: Option<ShellEnvironment>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSessionResult {
    pub zip_path: String,
    pub entries: Vec<String>,
    pub session_dir: String,
    pub manifest: ExportSessionManifest,
}
pub type SessionExportResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
#[async_trait]
pub trait SessionExportServiceContract: Send + Sync {
    async fn export(
        &self,
        input: ExportSessionPayload,
        options: Option<ExportSessionOptions>,
    ) -> SessionExportResult<ExportSessionResult>;
}
#[derive(Clone)]
pub struct SessionExportServiceHandle(pub Arc<dyn SessionExportServiceContract>);
impl Deref for SessionExportServiceHandle {
    type Target = dyn SessionExportServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
pub const SESSION_EXPORT_SERVICE_ID: ServiceIdentifier<SessionExportServiceHandle> =
    ServiceIdentifier::new("sessionExportService");
