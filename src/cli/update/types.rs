use serde::{Deserialize, Serialize};

// Original:
//   apps/kimi-code/src/cli/update/types.ts
pub const NPM_PACKAGE_NAME: &str = "@moonshot-ai/kimi-code";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallSource {
    NpmGlobal,
    PnpmGlobal,
    YarnGlobal,
    BunGlobal,
    Homebrew,
    Native,
    Unsupported,
}

impl InstallSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NpmGlobal => "npm-global",
            Self::PnpmGlobal => "pnpm-global",
            Self::YarnGlobal => "yarn-global",
            Self::BunGlobal => "bun-global",
            Self::Homebrew => "homebrew",
            Self::Native => "native",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateTarget {
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RolloutBatch {
    pub percent: u8,
    pub delay_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateManifest {
    pub version: String,
    pub published_at: String,
    #[serde(default)]
    pub rollout: Vec<RolloutBatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateCacheSource {
    Cdn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCache {
    pub source: UpdateCacheSource,
    pub checked_at: Option<String>,
    pub latest: Option<String>,
    pub manifest: Option<UpdateManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchLatestResult {
    pub latest: String,
    pub manifest: Option<UpdateManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInstallActive {
    pub version: String,
    pub source: InstallSource,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInstallFailure {
    pub version: String,
    pub failed_at: String,
    pub attempts: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInstallSuccess {
    pub version: String,
    pub installed_at: String,
    pub notified_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInstallState {
    pub active: Option<UpdateInstallActive>,
    pub last_failure: Option<UpdateInstallFailure>,
    pub last_success: Option<UpdateInstallSuccess>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateDecision {
    None,
    PromptInstall,
    ManualCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdatePreflightResult {
    Continue,
    Exit,
}

pub fn empty_update_cache() -> UpdateCache {
    UpdateCache {
        source: UpdateCacheSource::Cdn,
        checked_at: None,
        latest: None,
        manifest: None,
    }
}

pub fn empty_update_install_state() -> UpdateInstallState {
    UpdateInstallState {
        active: None,
        last_failure: None,
        last_success: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_empty_persistent_update_values() {
        assert_eq!(
            serde_json::to_value(empty_update_cache()).expect("cache json"),
            serde_json::json!({
                "source": "cdn",
                "checkedAt": null,
                "latest": null,
                "manifest": null
            })
        );
        assert_eq!(
            serde_json::to_value(empty_update_install_state()).expect("install state json"),
            serde_json::json!({
                "active": null,
                "lastFailure": null,
                "lastSuccess": null
            })
        );
    }

    #[test]
    fn preserves_manifest_and_install_source_json_shapes() {
        let manifest = UpdateManifest {
            version: "1.2.3".to_owned(),
            published_at: "2026-07-21T00:00:00.000Z".to_owned(),
            rollout: vec![RolloutBatch {
                percent: 25,
                delay_seconds: 3_600,
            }],
        };
        assert_eq!(
            serde_json::to_value(manifest).expect("manifest json"),
            serde_json::json!({
                "version": "1.2.3",
                "publishedAt": "2026-07-21T00:00:00.000Z",
                "rollout": [{ "percent": 25, "delaySeconds": 3600 }]
            })
        );
        assert_eq!(
            serde_json::to_value(InstallSource::PnpmGlobal).expect("source json"),
            "pnpm-global"
        );
    }
}
