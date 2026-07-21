use std::{
    error::Error,
    fmt,
    path::{Component, Path, PathBuf},
};

use futures_util::future::join_all;
use reqwest::{Client, StatusCode, redirect::Policy};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

pub const KIMI_CODE_PLUGIN_MARKETPLACE_URL: &str =
    "https://code.kimi.com/kimi-code/plugins/marketplace.json";
pub const KIMI_CODE_PLUGIN_MARKETPLACE_URL_ENV: &str = "KIMI_CODE_PLUGIN_MARKETPLACE_URL";

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketplaceLocation {
    Remote { raw: String, resolved: String },
    Local { raw: String, resolved: PathBuf },
}

impl MarketplaceLocation {
    fn resolved_label(&self) -> String {
        match self {
            Self::Remote { resolved, .. } => resolved.clone(),
            Self::Local { resolved, .. } => resolved.to_string_lossy().into_owned(),
        }
    }
}

#[derive(Debug)]
pub enum PluginMarketplaceError {
    EmptySource,
    InvalidFileUrl(String),
    Io(std::io::Error),
    Request(reqwest::Error),
    Http { status: StatusCode },
    InvalidJson(String),
    InvalidMarketplace(String),
}

impl fmt::Display for PluginMarketplaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySource => write!(
                formatter,
                "{KIMI_CODE_PLUGIN_MARKETPLACE_URL_ENV} cannot be empty."
            ),
            Self::InvalidFileUrl(value) => write!(formatter, "invalid file URL: {value}"),
            Self::Io(error) => error.fmt(formatter),
            Self::Request(error) => error.fmt(formatter),
            Self::Http { status } => {
                write!(formatter, "Plugin marketplace returned HTTP {status}")
            }
            Self::InvalidJson(message) => {
                write!(formatter, "Plugin marketplace is not valid JSON: {message}")
            }
            Self::InvalidMarketplace(message) => formatter.write_str(message),
        }
    }
}

impl Error for PluginMarketplaceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Request(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for PluginMarketplaceError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<reqwest::Error> for PluginMarketplaceError {
    fn from(value: reqwest::Error) -> Self {
        Self::Request(value)
    }
}

pub struct LoadPluginMarketplaceOptions<'a> {
    pub work_dir: &'a Path,
    pub source: Option<&'a str>,
}

/// Original: `utils/plugin-marketplace.ts`, `loadPluginMarketplace()`.
pub async fn load_plugin_marketplace(
    options: LoadPluginMarketplaceOptions<'_>,
) -> Result<PluginMarketplace, PluginMarketplaceError> {
    let environment_source = std::env::var(KIMI_CODE_PLUGIN_MARKETPLACE_URL_ENV).ok();
    let configured_source = options.source.or(environment_source.as_deref());
    let location = resolve_marketplace_location(
        configured_source.unwrap_or(KIMI_CODE_PLUGIN_MARKETPLACE_URL),
        options.work_dir,
    )?;
    let client = Client::new();
    let raw = match read_marketplace_text(&location, &client).await {
        Ok(raw) => raw,
        Err(error) if configured_source.is_none() => {
            let Some(fallback) = source_checkout_marketplace_location().await else {
                return Err(error);
            };
            let raw = read_marketplace_text(&fallback, &client).await?;
            let parsed = parse_plugin_marketplace(&raw, &fallback)?;
            return Ok(with_latest_versions(parsed).await);
        }
        Err(error) => return Err(error),
    };
    let parsed = parse_plugin_marketplace(&raw, &location)?;
    Ok(with_latest_versions(parsed).await)
}

async fn with_latest_versions(marketplace: PluginMarketplace) -> PluginMarketplace {
    let plugins = join_all(marketplace.plugins.into_iter().map(|entry| async move {
        if entry.version.is_some() {
            return entry;
        }
        let Some(latest) = resolve_latest_github_release(&entry.source).await else {
            return entry;
        };
        PluginMarketplaceEntry {
            version: Some(latest),
            ..entry
        }
    }))
    .await;
    PluginMarketplace {
        plugins,
        ..marketplace
    }
}

pub fn resolve_marketplace_location(
    source: &str,
    work_dir: &Path,
) -> Result<MarketplaceLocation, PluginMarketplaceError> {
    let source = source.trim();
    if source.is_empty() {
        return Err(PluginMarketplaceError::EmptySource);
    }
    if source.starts_with("http://") || source.starts_with("https://") {
        return Ok(MarketplaceLocation::Remote {
            raw: source.to_owned(),
            resolved: source.to_owned(),
        });
    }
    if source.starts_with("file://") {
        let url = Url::parse(source)
            .map_err(|_| PluginMarketplaceError::InvalidFileUrl(source.to_owned()))?;
        let path = url
            .to_file_path()
            .map_err(|_| PluginMarketplaceError::InvalidFileUrl(source.to_owned()))?;
        return Ok(MarketplaceLocation::Local {
            raw: source.to_owned(),
            resolved: path,
        });
    }
    Ok(MarketplaceLocation::Local {
        raw: source.to_owned(),
        resolved: resolve_local_path(source, work_dir),
    })
}

async fn source_checkout_marketplace_location() -> Option<MarketplaceLocation> {
    let resolved = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("plugins")
        .join("marketplace.json");
    tokio::fs::metadata(&resolved)
        .await
        .ok()
        .filter(|metadata| metadata.is_file())?;
    Some(MarketplaceLocation::Local {
        raw: resolved.to_string_lossy().into_owned(),
        resolved,
    })
}

async fn read_marketplace_text(
    location: &MarketplaceLocation,
    client: &Client,
) -> Result<String, PluginMarketplaceError> {
    match location {
        MarketplaceLocation::Local { resolved, .. } => {
            Ok(tokio::fs::read_to_string(resolved).await?)
        }
        MarketplaceLocation::Remote { resolved, .. } => {
            let response = client.get(resolved).send().await?;
            if !response.status().is_success() {
                return Err(PluginMarketplaceError::Http {
                    status: response.status(),
                });
            }
            Ok(response.text().await?)
        }
    }
}

/// Original: `utils/plugin-marketplace.ts`, `parsePluginMarketplace()`.
pub fn parse_plugin_marketplace(
    raw: &str,
    location: &MarketplaceLocation,
) -> Result<PluginMarketplace, PluginMarketplaceError> {
    let parsed = serde_json::from_str::<Value>(raw)
        .map_err(|error| PluginMarketplaceError::InvalidJson(error.to_string()))?;
    let object = parsed.as_object().ok_or_else(|| {
        PluginMarketplaceError::InvalidMarketplace(
            "Plugin marketplace must be an object.".to_owned(),
        )
    })?;
    let entries = object
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            PluginMarketplaceError::InvalidMarketplace(
                "Plugin marketplace must contain a \"plugins\" array.".to_owned(),
            )
        })?;
    let plugins = entries
        .iter()
        .enumerate()
        .map(|(index, value)| parse_marketplace_entry(value, index, location))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PluginMarketplace {
        source: location.resolved_label(),
        version: string_field(object, "version"),
        plugins,
    })
}

fn parse_marketplace_entry(
    value: &Value,
    index: usize,
    location: &MarketplaceLocation,
) -> Result<PluginMarketplaceEntry, PluginMarketplaceError> {
    let object = value.as_object().ok_or_else(|| {
        invalid(format!(
            "Plugin marketplace entry {} must be an object.",
            index + 1
        ))
    })?;
    let id = required_string(object, "id", index)?;
    validate_entry_type(object, &id)?;
    let source = string_field(object, "source")
        .or_else(|| string_field(object, "url"))
        .or_else(|| string_field(object, "downloadUrl"))
        .ok_or_else(|| {
            invalid(format!(
                "Plugin marketplace entry {id} must define \"source\"."
            ))
        })?;
    let source = resolve_entry_source(&source, location)?;
    Ok(PluginMarketplaceEntry {
        display_name: string_field(object, "displayName")
            .or_else(|| string_field(object, "name"))
            .unwrap_or_else(|| id.clone()),
        tier: parse_marketplace_tier(object, &id)?,
        version: string_field(object, "version")
            .or_else(|| derive_version_from_github_source(&source)),
        description: string_field(object, "description")
            .or_else(|| string_field(object, "shortDescription")),
        homepage: string_field(object, "homepage").or_else(|| string_field(object, "websiteURL")),
        keywords: string_array_field(object, "keywords"),
        id,
        source,
    })
}

fn validate_entry_type(
    object: &Map<String, Value>,
    id: &str,
) -> Result<(), PluginMarketplaceError> {
    let Some(raw) = object.get("type") else {
        return Ok(());
    };
    let Some(kind) = raw.as_str() else {
        return Err(invalid(format!(
            "Plugin marketplace entry {id} \"type\" must be a string."
        )));
    };
    if matches!(kind.trim(), "plugin" | "managed" | "guide") {
        Ok(())
    } else {
        Err(invalid(format!(
            "Plugin marketplace entry {id} \"type\" must be \"plugin\". Legacy aliases \"managed\" and \"guide\" are also accepted."
        )))
    }
}

fn parse_marketplace_tier(
    object: &Map<String, Value>,
    id: &str,
) -> Result<Option<PluginMarketplaceTier>, PluginMarketplaceError> {
    let Some(raw) = object.get("tier") else {
        return Ok(None);
    };
    let Some(tier) = raw.as_str() else {
        return Err(invalid(format!(
            "Plugin marketplace entry {id} \"tier\" must be a string."
        )));
    };
    match tier.trim() {
        "" => Ok(None),
        "official" => Ok(Some(PluginMarketplaceTier::Official)),
        "curated" => Ok(Some(PluginMarketplaceTier::Curated)),
        _ => Err(invalid(format!(
            "Plugin marketplace entry {id} \"tier\" must be one of: official, curated."
        ))),
    }
}

fn resolve_entry_source(
    source: &str,
    location: &MarketplaceLocation,
) -> Result<String, PluginMarketplaceError> {
    let source = source.trim();
    if source.starts_with("http://")
        || source.starts_with("https://")
        || source == "~"
        || source.starts_with("~/")
        || Path::new(source).is_absolute()
    {
        return Ok(source.to_owned());
    }
    if source.starts_with("file://") {
        let url = Url::parse(source)
            .map_err(|_| PluginMarketplaceError::InvalidFileUrl(source.to_owned()))?;
        return url
            .to_file_path()
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|_| PluginMarketplaceError::InvalidFileUrl(source.to_owned()));
    }
    match location {
        MarketplaceLocation::Remote { resolved, .. } => Url::parse(resolved)
            .and_then(|base| base.join(source))
            .map(|url| url.to_string())
            .map_err(|_| invalid(format!("Invalid plugin source: {source}"))),
        MarketplaceLocation::Local { resolved, .. } => Ok(lexical_normalize(
            &resolved
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(source),
        )
        .to_string_lossy()
        .into_owned()),
    }
}

pub fn derive_version_from_github_source(source: &str) -> Option<String> {
    let url = Url::parse(source).ok()?;
    if !matches!(url.host_str(), Some("github.com" | "www.github.com")) {
        return None;
    }
    let segments = url.path_segments()?.collect::<Vec<_>>();
    let reference = match segments.as_slice() {
        [_, _, "releases", "tag", reference, ..] => *reference,
        [_, _, "tree" | "commit", reference, ..] => *reference,
        _ => return None,
    };
    let decoded = percent_decode(reference);
    let candidate = decoded
        .strip_prefix('v')
        .or_else(|| decoded.strip_prefix('V'))
        .unwrap_or(&decoded);
    parse_node_semver(candidate).map(|version| version.to_string())
}

async fn resolve_latest_github_release(source: &str) -> Option<String> {
    let (owner, repo) = parse_github_repo(source)?;
    let client = Client::builder().redirect(Policy::none()).build().ok()?;
    let response = client
        .get(format!("https://github.com/{owner}/{repo}/releases/latest"))
        .send()
        .await
        .ok()?;
    if response.status() == StatusCode::NOT_FOUND {
        return None;
    }
    if !matches!(
        response.status(),
        StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND
    ) {
        return None;
    }
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)?
        .to_str()
        .ok()?;
    let tag = location
        .split("/releases/tag/")
        .nth(1)?
        .split(['/', '?', '#'])
        .next()?;
    let decoded = percent_decode(tag);
    let candidate = decoded
        .strip_prefix('v')
        .or_else(|| decoded.strip_prefix('V'))
        .unwrap_or(&decoded);
    parse_node_semver(candidate).map(|version| version.to_string())
}

fn parse_github_repo(source: &str) -> Option<(String, String)> {
    let url = Url::parse(source).ok()?;
    if !matches!(url.host_str(), Some("github.com" | "www.github.com")) {
        return None;
    }
    let segments = url
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    (segments.len() == 2).then(|| (segments[0].to_owned(), segments[1].to_owned()))
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            decoded.push(high * 16 + low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_owned())
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn resolve_local_path(input: &str, work_dir: &Path) -> PathBuf {
    if input == "~" {
        return dirs::home_dir().unwrap_or_default();
    }
    if let Some(relative) = input.strip_prefix("~/") {
        return dirs::home_dir().unwrap_or_default().join(relative);
    }
    let path = Path::new(input);
    if path.is_absolute() {
        path.to_owned()
    } else {
        lexical_normalize(&work_dir.join(path))
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            value => normalized.push(value.as_os_str()),
        }
    }
    normalized
}

fn required_string(
    object: &Map<String, Value>,
    field: &str,
    index: usize,
) -> Result<String, PluginMarketplaceError> {
    string_field(object, field).ok_or_else(|| {
        invalid(format!(
            "Plugin marketplace entry {} must define \"{field}\".",
            index + 1
        ))
    })
}

fn string_field(object: &Map<String, Value>, field: &str) -> Option<String> {
    object
        .get(field)?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn string_array_field(object: &Map<String, Value>, field: &str) -> Option<Vec<String>> {
    let values = object.get(field)?.as_array()?;
    let values = values
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

fn invalid(message: String) -> PluginMarketplaceError {
    PluginMarketplaceError::InvalidMarketplace(message)
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
    fn resolves_remote_file_and_relative_marketplace_locations() {
        assert!(matches!(
            resolve_marketplace_location("https://example.test/catalog.json", Path::new("/work")),
            Ok(MarketplaceLocation::Remote { resolved, .. })
                if resolved == "https://example.test/catalog.json"
        ));
        assert!(matches!(
            resolve_marketplace_location("catalog/../market.json", Path::new("/work")),
            Ok(MarketplaceLocation::Local { resolved, .. })
                if resolved == Path::new("/work/market.json")
        ));
        assert!(matches!(
            resolve_marketplace_location("   ", Path::new("/work")),
            Err(PluginMarketplaceError::EmptySource)
        ));
    }

    #[test]
    fn parses_alias_fields_relative_sources_tiers_and_github_versions() {
        let location = MarketplaceLocation::Remote {
            raw: "https://example.test/catalog/marketplace.json".to_owned(),
            resolved: "https://example.test/catalog/marketplace.json".to_owned(),
        };
        let marketplace = parse_plugin_marketplace(
            r#"{"version":"1","plugins":[{"id":"one","name":"One","url":"../one.zip","tier":"curated","shortDescription":"desc","websiteURL":"https://one.test","keywords":[" ai ",3,""]},{"id":"two","source":"https://github.com/acme/two/releases/tag/v2.3.4","type":"managed"}]}"#,
            &location,
        )
        .expect("valid marketplace");
        assert_eq!(marketplace.source, location.resolved_label());
        assert_eq!(marketplace.plugins[0].display_name, "One");
        assert_eq!(
            marketplace.plugins[0].source,
            "https://example.test/one.zip"
        );
        assert_eq!(marketplace.plugins[0].keywords, Some(vec!["ai".to_owned()]));
        assert_eq!(marketplace.plugins[1].version.as_deref(), Some("2.3.4"));
    }

    #[test]
    fn rejects_invalid_shapes_types_and_tiers_with_original_context() {
        let location = MarketplaceLocation::Local {
            raw: "market.json".to_owned(),
            resolved: PathBuf::from("/work/market.json"),
        };
        for (raw, expected) in [
            ("[]", "must be an object"),
            (r#"{}"#, "plugins\" array"),
            (r#"{"plugins":[{"source":"x"}]}"#, "must define \"id\""),
            (
                r#"{"plugins":[{"id":"x","source":"x","type":"other"}]}"#,
                "must be \"plugin\"",
            ),
            (
                r#"{"plugins":[{"id":"x","source":"x","tier":"other"}]}"#,
                "official, curated",
            ),
        ] {
            let error = parse_plugin_marketplace(raw, &location)
                .expect_err("invalid marketplace")
                .to_string();
            assert!(
                error.contains(expected),
                "{error:?} did not contain {expected:?}"
            );
        }
    }

    #[tokio::test]
    async fn loads_local_marketplace_asynchronously() {
        let directory =
            std::env::temp_dir().join(format!("kimi-marketplace-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&directory)
            .await
            .expect("create temp directory");
        let path = directory.join("marketplace.json");
        tokio::fs::write(
            &path,
            r#"{"plugins":[{"id":"local","source":"./local.zip","version":"1.0.0"}]}"#,
        )
        .await
        .expect("write marketplace");
        let marketplace = load_plugin_marketplace(LoadPluginMarketplaceOptions {
            work_dir: &directory,
            source: path.to_str(),
        })
        .await
        .expect("load marketplace");
        assert_eq!(marketplace.plugins[0].id, "local");
        assert_eq!(
            marketplace.plugins[0].source,
            directory.join("local.zip").to_string_lossy()
        );
        let _ = tokio::fs::remove_dir_all(directory).await;
    }

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
