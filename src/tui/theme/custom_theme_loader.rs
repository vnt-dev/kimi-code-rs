use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::utils::paths::{HomeDirectoryUnavailable, get_data_dir};

use super::colors::{ColorPalette, ResolvedTheme, get_built_in_palette};

const RESERVED_THEME_NAMES: [&str; 3] = ["dark", "light", "auto"];

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CustomThemeBase {
    Dark,
    Light,
}

impl From<CustomThemeBase> for ResolvedTheme {
    fn from(value: CustomThemeBase) -> Self {
        match value {
            CustomThemeBase::Dark => Self::Dark,
            CustomThemeBase::Light => Self::Light,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CustomThemeDefinition {
    name: String,
    #[serde(default)]
    _display_name: Option<String>,
    #[serde(default)]
    base: Option<CustomThemeBase>,
    #[serde(default)]
    colors: HashMap<String, String>,
}

#[derive(Debug)]
struct ParsedCustomTheme {
    base: ResolvedTheme,
    colors: HashMap<String, String>,
}

/// Original: `custom-theme-loader.ts`, `getCustomThemesDir()`.
pub fn get_custom_themes_dir() -> Result<PathBuf, HomeDirectoryUnavailable> {
    Ok(get_data_dir()?.join("themes"))
}

/// Loads only explicitly configured, valid `#RRGGBB` color entries.
pub async fn load_custom_theme(name: &str) -> Option<HashMap<String, String>> {
    let directory = get_custom_themes_dir().ok()?;
    read_custom_theme(&directory, name)
        .await
        .map(|theme| theme.colors)
}

/// Loads a custom theme and overlays its recognized tokens on the selected
/// built-in base palette (dark by default).
pub async fn load_custom_theme_merged(name: &str) -> Option<ColorPalette> {
    let directory = get_custom_themes_dir().ok()?;
    load_custom_theme_merged_from(&directory, name).await
}

pub async fn list_custom_themes() -> Vec<String> {
    let Ok(directory) = get_custom_themes_dir() else {
        return Vec::new();
    };
    list_custom_themes_from(&directory).await
}

/// Synchronous listing for modal construction paths that cannot await.
pub fn list_custom_themes_sync() -> Vec<String> {
    let Ok(directory) = get_custom_themes_dir() else {
        return Vec::new();
    };
    list_custom_themes_sync_from(&directory)
}

async fn read_custom_theme(directory: &Path, name: &str) -> Option<ParsedCustomTheme> {
    let content = tokio::fs::read_to_string(directory.join(format!("{name}.json")))
        .await
        .ok()?;
    parse_custom_theme(&content)
}

fn parse_custom_theme(content: &str) -> Option<ParsedCustomTheme> {
    let definition = serde_json::from_str::<CustomThemeDefinition>(content).ok()?;
    if definition.name.is_empty() {
        return None;
    }
    let colors = definition
        .colors
        .into_iter()
        .filter(|(_, value)| is_hex_color(value))
        .collect();
    Some(ParsedCustomTheme {
        base: definition.base.unwrap_or(CustomThemeBase::Dark).into(),
        colors,
    })
}

async fn load_custom_theme_merged_from(directory: &Path, name: &str) -> Option<ColorPalette> {
    let theme = read_custom_theme(directory, name).await?;
    let mut palette = get_built_in_palette(theme.base);
    apply_colors(&mut palette, &theme.colors);
    Some(palette)
}

async fn list_custom_themes_from(directory: &Path) -> Vec<String> {
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return Vec::new();
    };
    let mut files = Vec::new();
    loop {
        match entries.next_entry().await {
            Ok(Some(entry)) => {
                if entry.file_type().await.is_ok_and(|kind| kind.is_file()) {
                    files.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
            Ok(None) => break,
            Err(_) => return Vec::new(),
        }
    }
    to_theme_names(files)
}

fn list_custom_themes_sync_from(directory: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let files = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    to_theme_names(files)
}

fn to_theme_names(files: Vec<String>) -> Vec<String> {
    files
        .into_iter()
        .filter_map(|file| file.strip_suffix(".json").map(str::to_owned))
        .filter(|name| !RESERVED_THEME_NAMES.contains(&name.as_str()))
        .collect()
}

fn is_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn apply_colors(palette: &mut ColorPalette, colors: &HashMap<String, String>) {
    for (name, value) in colors {
        let target = match name.as_str() {
            "primary" => &mut palette.primary,
            "accent" => &mut palette.accent,
            "text" => &mut palette.text,
            "textStrong" => &mut palette.text_strong,
            "textDim" => &mut palette.text_dim,
            "textMuted" => &mut palette.text_muted,
            "border" => &mut palette.border,
            "borderFocus" => &mut palette.border_focus,
            "success" => &mut palette.success,
            "warning" => &mut palette.warning,
            "error" => &mut palette.error,
            "diffAdded" => &mut palette.diff_added,
            "diffRemoved" => &mut palette.diff_removed,
            "diffAddedStrong" => &mut palette.diff_added_strong,
            "diffRemovedStrong" => &mut palette.diff_removed_strong,
            "diffGutter" => &mut palette.diff_gutter,
            "diffMeta" => &mut palette.diff_meta,
            "roleUser" => &mut palette.role_user,
            "shellMode" => &mut palette.shell_mode,
            _ => continue,
        };
        target.clone_from(value);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "kimi-code-rs-custom-theme-{}-{id}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self(path)
        }

        fn write(&self, name: &str, content: &str) {
            fs::write(self.0.join(name), content).expect("write theme");
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn loads_valid_colors_and_merges_onto_requested_base() {
        let directory = TestDirectory::new();
        directory.write(
            "ocean.json",
            r##"{
                "name":"ocean",
                "displayName":"Ocean",
                "base":"light",
                "colors":{
                    "primary":"#ABCDEF",
                    "textDim":"invalid",
                    "shellMode":"#123456",
                    "unknown":"#111111"
                }
            }"##,
        );
        let parsed = read_custom_theme(&directory.0, "ocean")
            .await
            .expect("theme");
        assert_eq!(parsed.colors["primary"], "#ABCDEF");
        assert!(!parsed.colors.contains_key("textDim"));
        assert_eq!(parsed.colors["unknown"], "#111111");

        let merged = load_custom_theme_merged_from(&directory.0, "ocean")
            .await
            .expect("merged");
        assert_eq!(merged.primary, "#ABCDEF");
        assert_eq!(merged.shell_mode, "#123456");
        assert_eq!(
            merged.text_dim,
            get_built_in_palette(ResolvedTheme::Light).text_dim
        );
    }

    #[tokio::test]
    async fn rejects_invalid_schema_missing_files_and_empty_names() {
        let directory = TestDirectory::new();
        directory.write("empty.json", r#"{"name":"","colors":{}}"#);
        directory.write("bad-base.json", r#"{"name":"bad","base":"blue"}"#);
        directory.write("syntax.json", "{");
        for name in ["empty", "bad-base", "syntax", "missing"] {
            assert!(read_custom_theme(&directory.0, name).await.is_none());
        }
    }

    #[tokio::test]
    async fn lists_json_files_and_hides_reserved_or_directory_entries() {
        let directory = TestDirectory::new();
        for name in [
            "ocean.json",
            "warm.json",
            "dark.json",
            "light.json",
            "auto.json",
        ] {
            directory.write(name, r#"{"name":"test"}"#);
        }
        directory.write("notes.txt", "ignored");
        fs::create_dir(directory.0.join("folder.json")).expect("directory entry");

        let mut asynchronous = list_custom_themes_from(&directory.0).await;
        asynchronous.sort();
        let mut synchronous = list_custom_themes_sync_from(&directory.0);
        synchronous.sort();
        assert_eq!(asynchronous, ["ocean", "warm"]);
        assert_eq!(synchronous, asynchronous);
    }

    #[test]
    fn missing_theme_directory_lists_as_empty() {
        let missing = std::env::temp_dir().join(format!(
            "kimi-code-rs-missing-theme-dir-{}",
            std::process::id()
        ));
        assert!(list_custom_themes_sync_from(&missing).is_empty());
    }
}
