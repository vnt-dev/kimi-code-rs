use semver::Version;
use serde::{Deserialize, Serialize};

pub const PLUGIN_MARKETPLACE_TIERS: [PluginMarketplaceTier; 2] = [
    PluginMarketplaceTier::Official,
    PluginMarketplaceTier::Curated,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginMarketplaceTier {
    Official,
    Curated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginMarketplaceEntry {
    pub id: String,
    pub display_name: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<PluginMarketplaceTier>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMarketplace {
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub plugins: Vec<PluginMarketplaceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginUpdateStatus {
    NotInstalled,
    UpToDate { version: Option<String> },
    Update { local: String, latest: String },
}

/// Original: `utils/plugin-marketplace.ts`, `computeUpdateStatus()`.
pub fn compute_update_status(
    latest: Option<&str>,
    local: Option<&str>,
    installed: bool,
) -> PluginUpdateStatus {
    if !installed {
        return PluginUpdateStatus::NotInstalled;
    }
    if let (Some(latest), Some(local), Some(latest_version), Some(local_version)) = (
        latest,
        local,
        latest.and_then(parse_node_semver),
        local.and_then(parse_node_semver),
    ) && latest_version > local_version
    {
        return PluginUpdateStatus::Update {
            local: local.to_owned(),
            latest: latest.to_owned(),
        };
    }
    PluginUpdateStatus::UpToDate {
        version: local.map(str::to_owned),
    }
}

fn parse_node_semver(value: &str) -> Option<Version> {
    let normalized = value
        .strip_prefix('v')
        .or_else(|| value.strip_prefix('V'))
        .unwrap_or(value);
    Version::parse(normalized).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_not_installed_without_comparing_versions() {
        assert_eq!(
            compute_update_status(Some("2.0.0"), Some("1.0.0"), false),
            PluginUpdateStatus::NotInstalled
        );
    }

    #[test]
    fn reports_only_strict_valid_semver_upgrades() {
        assert_eq!(
            compute_update_status(Some("v2.0.0"), Some("1.5.0"), true),
            PluginUpdateStatus::Update {
                local: "1.5.0".to_owned(),
                latest: "v2.0.0".to_owned()
            }
        );
        for (latest, local) in [
            (Some("1.0.0"), Some("1.0.0")),
            (Some("0.9.0"), Some("1.0.0")),
            (Some("latest"), Some("1.0.0")),
            (Some("2.0.0"), Some("unknown")),
        ] {
            assert!(matches!(
                compute_update_status(latest, local, true),
                PluginUpdateStatus::UpToDate { .. }
            ));
        }
    }

    #[test]
    fn up_to_date_status_never_borrows_the_marketplace_version() {
        assert_eq!(
            compute_update_status(Some("2.0.0"), None, true),
            PluginUpdateStatus::UpToDate { version: None }
        );
    }
}
