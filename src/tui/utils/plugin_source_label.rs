use url::Url;

use crate::sdk::types::{PluginInfo, PluginSource, PluginSummary};

pub const OFFICIAL_BADGE: &str = "official";
pub const CURATED_BADGE: &str = "curated";
pub const THIRD_PARTY_BADGE: &str = "third-party";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginTrustLabel {
    Official,
    Curated,
    ThirdParty,
}

pub trait PluginTrustMetadata {
    fn source(&self) -> PluginSource;
    fn original_source(&self) -> Option<&str>;
}

impl PluginTrustMetadata for PluginSummary {
    fn source(&self) -> PluginSource {
        self.source
    }

    fn original_source(&self) -> Option<&str> {
        self.original_source.as_deref()
    }
}

impl PluginTrustMetadata for PluginInfo {
    fn source(&self) -> PluginSource {
        self.source
    }

    fn original_source(&self) -> Option<&str> {
        self.original_source.as_deref()
    }
}

impl PluginTrustLabel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Official => OFFICIAL_BADGE,
            Self::Curated => CURATED_BADGE,
            Self::ThirdParty => THIRD_PARTY_BADGE,
        }
    }
}

/// Original:
///   apps/kimi-code/src/tui/utils/plugin-source-label.ts
///   formatPluginSourceLabel()
pub fn format_plugin_source_label(plugin: &PluginSummary) -> String {
    if plugin.source == PluginSource::Github
        && let Some(github) = &plugin.github
    {
        return format!(
            "github {}/{}@{}",
            github.owner, github.repo, github.reference.value
        );
    }
    if plugin.source == PluginSource::ZipUrl
        && let Some(host) = plugin.original_source.as_deref().and_then(host_from_url)
    {
        return format!("via {host}");
    }
    plugin.source.as_str().to_owned()
}

/// Original:
///   apps/kimi-code/src/tui/utils/plugin-source-label.ts
///   pluginTrustLabel()
pub fn plugin_trust_label(plugin: &impl PluginTrustMetadata) -> PluginTrustLabel {
    if plugin.source() != PluginSource::ZipUrl {
        return PluginTrustLabel::ThirdParty;
    }
    let Some(url) = plugin.original_source().and_then(parse_url) else {
        return PluginTrustLabel::ThirdParty;
    };
    if url.scheme() != "https" || url.host_str() != Some("code.kimi.com") {
        return PluginTrustLabel::ThirdParty;
    }
    if url.path().starts_with("/kimi-code/plugins/official/") {
        PluginTrustLabel::Official
    } else if url.path().starts_with("/kimi-code/plugins/curated/") {
        PluginTrustLabel::Curated
    } else {
        PluginTrustLabel::ThirdParty
    }
}

/// Original:
///   apps/kimi-code/src/tui/utils/plugin-source-label.ts
///   isOfficialPluginSource()
pub fn is_official_plugin_source(source: &str) -> bool {
    let trimmed = source.trim();
    if !trimmed.starts_with("https://") {
        return false;
    }
    let Some(url) = parse_url(trimmed) else {
        return false;
    };
    url.host_str() == Some("code.kimi.com")
        && url.path().starts_with("/kimi-code/plugins/official/")
}

fn parse_url(raw: &str) -> Option<Url> {
    Url::parse(raw).ok()
}

fn host_from_url(raw: &str) -> Option<String> {
    let url = parse_url(raw)?;
    let host = url.host_str()?;
    match url.port() {
        Some(port) => Some(format!("{host}:{port}")),
        None => Some(host.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::types::{
        PluginGithubMetadata, PluginGithubRef, PluginGithubRefKind, PluginState,
    };

    fn plugin(source: PluginSource, original_source: Option<&str>) -> PluginSummary {
        PluginSummary {
            id: "demo".to_owned(),
            display_name: "Demo".to_owned(),
            version: None,
            enabled: true,
            state: PluginState::Ok,
            skill_count: 0,
            mcp_server_count: 0,
            enabled_mcp_server_count: 0,
            hook_count: 0,
            command_count: 0,
            has_errors: false,
            source,
            original_source: original_source.map(str::to_owned),
            github: None,
        }
    }

    #[test]
    fn formats_github_zip_and_fallback_sources() {
        let mut github = plugin(PluginSource::Github, None);
        github.github = Some(PluginGithubMetadata {
            owner: "moonshotai".to_owned(),
            repo: "demo".to_owned(),
            reference: PluginGithubRef {
                kind: PluginGithubRefKind::Branch,
                value: "main".to_owned(),
            },
            installed_sha: None,
        });
        assert_eq!(
            format_plugin_source_label(&github),
            "github moonshotai/demo@main"
        );
        assert_eq!(
            format_plugin_source_label(&plugin(
                PluginSource::ZipUrl,
                Some("https://plugins.example:8443/demo.zip")
            )),
            "via plugins.example:8443"
        );
        assert_eq!(
            format_plugin_source_label(&plugin(PluginSource::LocalPath, None)),
            "local-path"
        );
        assert_eq!(
            format_plugin_source_label(&plugin(PluginSource::ZipUrl, Some("not a URL"))),
            "zip-url"
        );
    }

    #[test]
    fn labels_only_kimi_https_plugin_paths_as_trusted() {
        let official = plugin(
            PluginSource::ZipUrl,
            Some("https://code.kimi.com/kimi-code/plugins/official/demo.zip"),
        );
        let curated = plugin(
            PluginSource::ZipUrl,
            Some("https://code.kimi.com/kimi-code/plugins/curated/demo.zip"),
        );
        assert_eq!(plugin_trust_label(&official), PluginTrustLabel::Official);
        assert_eq!(plugin_trust_label(&curated), PluginTrustLabel::Curated);
        assert_eq!(
            plugin_trust_label(&plugin(
                PluginSource::Github,
                Some("https://code.kimi.com/kimi-code/plugins/official/demo.zip")
            )),
            PluginTrustLabel::ThirdParty
        );
        assert_eq!(PluginTrustLabel::Official.as_str(), OFFICIAL_BADGE);
    }

    #[test]
    fn recognizes_only_unambiguous_official_install_urls() {
        assert!(is_official_plugin_source(
            "  https://code.kimi.com/kimi-code/plugins/official/demo.zip  "
        ));
        assert!(!is_official_plugin_source(
            "http://code.kimi.com/kimi-code/plugins/official/demo.zip"
        ));
        assert!(!is_official_plugin_source(
            "https://code.kimi.com/kimi-code/plugins/curated/demo.zip"
        ));
        assert!(!is_official_plugin_source(
            "HTTPS://code.kimi.com/kimi-code/plugins/official/demo.zip"
        ));
        assert!(!is_official_plugin_source("not a URL"));
    }
}
