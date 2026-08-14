//! Export manifest construction.
//! Original: `packages/agent-core-v2/src/app/sessionExport/manifest.ts`.
use super::{ExportSessionManifest, ShellEnvironment};
use crate::wire::migration::WIRE_PROTOCOL_VERSION;
use chrono::{DateTime, Utc};
#[derive(Clone, Debug, Default)]
pub struct SessionWireScan {
    pub first_activity_ms: Option<i64>,
    pub last_activity_ms: Option<i64>,
    pub last_user_message_ms: Option<i64>,
    pub first_user_input: Option<String>,
}
#[derive(Clone, Debug)]
pub struct ExportSessionManifestSummary {
    pub id: String,
    pub title: Option<String>,
    pub workspace_dir: Option<String>,
}
pub struct BuildExportManifestArgs {
    pub summary: ExportSessionManifestSummary,
    pub now: DateTime<Utc>,
    pub version: String,
    pub wire_protocol_version: Option<String>,
    pub session_scan: SessionWireScan,
    pub session_log_path: Option<String>,
    pub global_log_path: Option<String>,
    pub web_log_path: Option<String>,
    pub install_source: Option<String>,
    pub shell_env: Option<ShellEnvironment>,
    pub nodejs_version: String,
}
pub fn build_export_manifest(args: BuildExportManifestArgs) -> ExportSessionManifest {
    ExportSessionManifest {
        session_id: args.summary.id,
        exported_at: args
            .now
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        kimi_code_version: args.version,
        wire_protocol_version: args
            .wire_protocol_version
            .unwrap_or_else(|| WIRE_PROTOCOL_VERSION.into()),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        nodejs_version: args.nodejs_version,
        session_first_activity: iso(args.session_scan.first_activity_ms),
        session_last_activity: iso(args.session_scan.last_activity_ms),
        title: args.summary.title,
        workspace_dir: args.summary.workspace_dir,
        session_log_path: args.session_log_path,
        global_log_path: args.global_log_path,
        web_log_path: args.web_log_path,
        install_source: args.install_source,
        shell_env: args.shell_env,
    }
}
fn iso(value: Option<i64>) -> Option<String> {
    value
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_uses_wire_default_and_activity_timestamps() {
        let manifest = build_export_manifest(BuildExportManifestArgs {
            summary: ExportSessionManifestSummary {
                id: "s".into(),
                title: None,
                workspace_dir: None,
            },
            now: DateTime::from_timestamp(0, 0).unwrap(),
            version: "v".into(),
            wire_protocol_version: None,
            session_scan: SessionWireScan {
                first_activity_ms: Some(1000),
                ..Default::default()
            },
            session_log_path: None,
            global_log_path: None,
            web_log_path: None,
            install_source: None,
            shell_env: None,
            nodejs_version: "20.0.0".into(),
        });
        assert_eq!(manifest.wire_protocol_version, WIRE_PROTOCOL_VERSION);
        assert_eq!(
            manifest.session_first_activity.as_deref(),
            Some("1970-01-01T00:00:01.000Z")
        );
    }
}
