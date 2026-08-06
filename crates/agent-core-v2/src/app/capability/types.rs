//! `capability` domain types — built-in product capabilities (kimi-cu,
//! kimi-webbridge) that bundle a binary runtime + agent wiring + manual
//! user steps. A capability is NOT a plugin: plugins are declarative
//! contributions to a session, while capabilities own imperative install
//! orchestration and a layered readiness state machine for product-specific
//! runtimes (macOS app + launchd service + TCC permissions; Windows signed
//! runtime; local HTTP daemon + browser extension). Steps marked `optional`
//! never block `ready`; `install.step` is a machine key clients localize.
//!
//! Original: `packages/agent-core-v2/src/app/capability/types.ts`.

use std::{error::Error, fmt};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum CapabilityId {
    #[serde(rename = "kimi-cu")]
    KimiCu,
    #[serde(rename = "kimi-webbridge")]
    KimiWebbridge,
}

impl CapabilityId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::KimiCu => "kimi-cu",
            Self::KimiWebbridge => "kimi-webbridge",
        }
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl TryFrom<&str> for CapabilityId {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "kimi-cu" => Ok(Self::KimiCu),
            "kimi-webbridge" => Ok(Self::KimiWebbridge),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityReadiness {
    NotInstalled,
    Partial,
    Ready,
    Unsupported,
}

impl CapabilityReadiness {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Partial => "partial",
            Self::Ready => "ready",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityStepState {
    Ok,
    Missing,
    Failed,
}

impl CapabilityStepState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Missing => "missing",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapabilityStep {
    pub id: String,
    pub state: CapabilityStepState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optional: Option<bool>,
}

impl CapabilityStep {
    pub fn new(id: impl Into<String>, state: CapabilityStepState) -> Self {
        Self {
            id: id.into(),
            state,
            detail: None,
            optional: None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapabilityInstallProgress {
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapabilityDetectResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub steps: Vec<CapabilityStep>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityStatus {
    pub id: CapabilityId,
    /// Plugin identifier used to provide this capability's agent wiring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    pub display_name: String,
    pub description: String,
    pub supported: bool,
    pub state: CapabilityReadiness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub steps: Vec<CapabilityStep>,
    pub install: CapabilityInstallProgress,
}

pub type CapabilityEntryError = Box<dyn Error + Send + Sync>;
pub type CapabilityEntryResult<T> = Result<T, CapabilityEntryError>;

/// Original: `CapabilityInstallReporter = (step: string, percent?: number) => void`.
pub type CapabilityInstallReporter = Box<dyn Fn(&str, Option<u32>) + Send + Sync>;

/// Original: `CapabilityEntry`. Entries are constructed with their host
/// context and hold only per-entry state; detection and install are
/// idempotent and safe to invoke repeatedly.
#[async_trait]
pub trait CapabilityEntry: Send + Sync {
    fn id(&self) -> CapabilityId;
    fn plugin_id(&self) -> Option<&str>;
    fn display_name(&self) -> &str;
    fn description(&self) -> &str;
    fn supported(&self) -> bool;
    async fn detect(&self) -> CapabilityEntryResult<CapabilityDetectResult>;
    async fn install(&self, report: CapabilityInstallReporter) -> CapabilityEntryResult<()>;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn wire_names_match_the_original() {
        assert_eq!(
            serde_json::to_value(CapabilityId::KimiCu).unwrap(),
            json!("kimi-cu")
        );
        assert_eq!(
            serde_json::to_value(CapabilityId::KimiWebbridge).unwrap(),
            json!("kimi-webbridge")
        );
        assert_eq!(
            serde_json::to_value(CapabilityReadiness::NotInstalled).unwrap(),
            json!("not_installed")
        );
        assert_eq!(
            serde_json::to_value(CapabilityStepState::Missing).unwrap(),
            json!("missing")
        );
        assert_eq!(CapabilityId::try_from("kimi-cu"), Ok(CapabilityId::KimiCu));
        assert!(CapabilityId::try_from("nope").is_err());
        assert_eq!(
            serde_json::to_value(CapabilityStatus {
                id: CapabilityId::KimiCu,
                plugin_id: Some("kimi-cu-win".to_owned()),
                display_name: "Kimi Computer Use".to_owned(),
                description: "demo".to_owned(),
                supported: true,
                state: CapabilityReadiness::Ready,
                version: None,
                steps: vec![CapabilityStep {
                    id: "plugin".to_owned(),
                    state: CapabilityStepState::Ok,
                    detail: None,
                    optional: None,
                }],
                install: CapabilityInstallProgress {
                    running: false,
                    step: None,
                    percent: None,
                    error: None,
                },
            })
            .unwrap(),
            json!({
                "id": "kimi-cu",
                "pluginId": "kimi-cu-win",
                "displayName": "Kimi Computer Use",
                "description": "demo",
                "supported": true,
                "state": "ready",
                "steps": [{"id": "plugin", "state": "ok"}],
                "install": {"running": false},
            })
        );
    }
}
