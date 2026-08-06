use std::{env, fs, path::PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};
use url::Url;

const DEFAULT_MARKETPLACE_URL: &str = "https://code.kimi.com/kimi-code/plugins/marketplace.json";
const MARKETPLACE_URL_ENV: &str = "KIMI_CODE_PLUGIN_MARKETPLACE_URL";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginMarketplaceEntry {
    pub id: String,
    pub display_name: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginMarketplace {
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub plugins: Vec<PluginMarketplaceEntry>,
}

enum MarketplaceLocation {
    Remote(Url),
    Local(PathBuf),
}

impl MarketplaceLocation {
    fn display(&self) -> String {
        match self {
            Self::Remote(url) => url.to_string(),
            Self::Local(path) => path.to_string_lossy().into_owned(),
        }
    }

    fn resolve_entry_source(&self, source: &str) -> Result<String, String> {
        let source = source.trim();
        if source.is_empty() {
            return Err("plugin marketplace entry source cannot be empty".into());
        }
        if source.starts_with("http://") || source.starts_with("https://") {
            return Ok(source.to_owned());
        }
        if source.starts_with("file://") {
            return Url::parse(source)
                .map_err(|error| error.to_string())?
                .to_file_path()
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|_| format!("invalid file URL `{source}`"));
        }
        let path = PathBuf::from(source);
        if path.is_absolute() {
            return Ok(path.to_string_lossy().into_owned());
        }
        match self {
            Self::Remote(url) => url
                .join(source)
                .map(|url| url.to_string())
                .map_err(|error| format!("failed to resolve plugin source `{source}`: {error}")),
            Self::Local(path) => Ok(path
                .parent()
                .unwrap_or(path.as_path())
                .join(source)
                .to_string_lossy()
                .into_owned()),
        }
    }
}

pub async fn load_plugin_marketplace() -> Result<PluginMarketplace, String> {
    let source = env::var(MARKETPLACE_URL_ENV)
        .ok()
        .unwrap_or_else(|| DEFAULT_MARKETPLACE_URL.to_owned());
    load_plugin_marketplace_from(&source).await
}

async fn load_plugin_marketplace_from(source: &str) -> Result<PluginMarketplace, String> {
    let location = resolve_location(source)?;
    let raw = match &location {
        MarketplaceLocation::Remote(url) => {
            let response = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .map_err(|error| error.to_string())?
                .get(url.clone())
                .send()
                .await
                .map_err(|error| format!("failed to load plugin marketplace: {error}"))?;
            if !response.status().is_success() {
                return Err(format!(
                    "plugin marketplace returned HTTP {}",
                    response.status().as_u16()
                ));
            }
            response
                .text()
                .await
                .map_err(|error| format!("failed to read plugin marketplace: {error}"))?
        }
        MarketplaceLocation::Local(path) => fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?,
    };
    parse_marketplace(&raw, &location)
}

fn resolve_location(source: &str) -> Result<MarketplaceLocation, String> {
    let source = source.trim();
    if source.is_empty() {
        return Err(format!("{MARKETPLACE_URL_ENV} cannot be empty"));
    }
    if source.starts_with("http://") || source.starts_with("https://") {
        return Url::parse(source)
            .map(MarketplaceLocation::Remote)
            .map_err(|error| format!("invalid plugin marketplace URL: {error}"));
    }
    if source.starts_with("file://") {
        return Url::parse(source)
            .map_err(|error| format!("invalid plugin marketplace file URL: {error}"))?
            .to_file_path()
            .map(MarketplaceLocation::Local)
            .map_err(|_| "invalid plugin marketplace file URL".to_owned());
    }
    let path = PathBuf::from(source);
    let path = if path.is_absolute() {
        path
    } else {
        env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    Ok(MarketplaceLocation::Local(path))
}

fn parse_marketplace(
    raw: &str,
    location: &MarketplaceLocation,
) -> Result<PluginMarketplace, String> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|error| format!("plugin marketplace is not valid JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "plugin marketplace must be an object".to_owned())?;
    let plugins = object
        .get("plugins")
        .and_then(Value::as_array)
        .ok_or_else(|| "plugin marketplace must contain a `plugins` array".to_owned())?;
    let plugins = plugins
        .iter()
        .enumerate()
        .map(|(index, entry)| parse_entry(entry, index, location))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PluginMarketplace {
        source: location.display(),
        version: string_field(object, "version"),
        plugins,
    })
}

fn parse_entry(
    value: &Value,
    index: usize,
    location: &MarketplaceLocation,
) -> Result<PluginMarketplaceEntry, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("plugin marketplace entry {} must be an object", index + 1))?;
    let id = string_field(object, "id")
        .ok_or_else(|| format!("plugin marketplace entry {} must define `id`", index + 1))?;
    if let Some(kind) = string_field(object, "type")
        && !matches!(kind.as_str(), "plugin" | "managed" | "guide")
    {
        return Err(format!(
            "plugin marketplace entry `{id}` has invalid type `{kind}`"
        ));
    }
    let tier = string_field(object, "tier");
    if tier
        .as_deref()
        .is_some_and(|tier| !matches!(tier, "official" | "curated"))
    {
        return Err(format!("plugin marketplace entry `{id}` has invalid tier"));
    }
    let raw_source = string_field(object, "source")
        .or_else(|| string_field(object, "url"))
        .or_else(|| string_field(object, "downloadUrl"))
        .ok_or_else(|| format!("plugin marketplace entry `{id}` must define `source`"))?;
    Ok(PluginMarketplaceEntry {
        display_name: string_field(object, "displayName")
            .or_else(|| string_field(object, "name"))
            .unwrap_or_else(|| id.clone()),
        source: location.resolve_entry_source(&raw_source)?,
        tier,
        version: string_field(object, "version"),
        description: string_field(object, "description")
            .or_else(|| string_field(object, "shortDescription")),
        homepage: string_field(object, "homepage").or_else(|| string_field(object, "websiteURL")),
        keywords: string_array_field(object, "keywords"),
        id,
    })
}

fn string_field(object: &Map<String, Value>, field: &str) -> Option<String> {
    object
        .get(field)
        .and_then(Value::as_str)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_aliases_and_resolves_remote_sources() {
        let location = MarketplaceLocation::Remote(
            Url::parse("https://example.test/plugins/marketplace.json").unwrap(),
        );
        let parsed = parse_marketplace(
            r#"{"version":"1","plugins":[{"id":"demo","name":"Demo","type":"managed","tier":"official","downloadUrl":"./demo.zip","shortDescription":"Useful","websiteURL":"https://example.test","keywords":[" one ",1]}]}"#,
            &location,
        )
        .unwrap();
        assert_eq!(parsed.plugins[0].display_name, "Demo");
        assert_eq!(
            parsed.plugins[0].source,
            "https://example.test/plugins/demo.zip"
        );
        assert_eq!(parsed.plugins[0].keywords, Some(vec!["one".into()]));
    }

    #[test]
    fn rejects_invalid_shape_and_tier() {
        let location = MarketplaceLocation::Local(PathBuf::from("C:/plugins/marketplace.json"));
        assert!(
            parse_marketplace("[]", &location)
                .unwrap_err()
                .contains("object")
        );
        assert!(
            parse_marketplace(
                r#"{"plugins":[{"id":"demo","source":"./demo","tier":"unknown"}]}"#,
                &location,
            )
            .unwrap_err()
            .contains("invalid tier")
        );
    }

    #[test]
    fn resolves_relative_sources_next_to_local_marketplace() {
        let location = MarketplaceLocation::Local(PathBuf::from("C:/plugins/marketplace.json"));
        let parsed = parse_marketplace(
            r#"{"plugins":[{"id":"demo","source":"./official/demo"}]}"#,
            &location,
        )
        .unwrap();
        assert_eq!(
            PathBuf::from(&parsed.plugins[0].source),
            PathBuf::from("C:/plugins/./official/demo")
        );
    }
}
