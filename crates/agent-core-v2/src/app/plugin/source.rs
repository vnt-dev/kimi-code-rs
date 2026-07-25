//! Plugin install-source recognition.
//!
//! Original: `packages/agent-core-v2/src/app/plugin/source.ts`.

use std::sync::LazyLock;

use percent_encoding::percent_decode_str;
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use super::types::{PluginGithubRef, PluginGithubRefKind};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ResolvedSource {
    LocalPath {
        path: String,
    },
    ZipUrl {
        path: String,
    },
    Github {
        owner: String,
        repo: String,
        #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
        reference: Option<PluginGithubRef>,
    },
}

pub type InstallSource = ResolvedSource;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ResolveInstallSourceError {
    #[error("Plugin root must be an absolute path (got \"{input}\")")]
    RelativeLocalPath { input: String },
}

static SHA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{7,40}$").expect("SHA regex must compile"));

// Original: source.ts, resolveInstallSource().
pub fn resolve_install_source(source: &str) -> Result<ResolvedSource, ResolveInstallSourceError> {
    let trimmed = source.trim();

    if let Some(github) = parse_github_url(trimmed) {
        return Ok(github);
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Ok(ResolvedSource::ZipUrl {
            path: trimmed.to_owned(),
        });
    }

    if !std::path::Path::new(trimmed).is_absolute() {
        return Err(ResolveInstallSourceError::RelativeLocalPath {
            input: source.to_owned(),
        });
    }

    Ok(ResolvedSource::LocalPath {
        path: trimmed.to_owned(),
    })
}

// Original: source.ts, parseGithubUrl().
fn parse_github_url(raw: &str) -> Option<ResolvedSource> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    if !matches!(url.host_str(), Some("github.com" | "www.github.com")) {
        return None;
    }

    let segments: Vec<_> = url
        .path()
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let owner = *segments.first()?;
    let repo_raw = *segments.get(1)?;
    let repo = repo_raw.strip_suffix(".git").unwrap_or(repo_raw);
    let rest = &segments[2..];

    if rest.is_empty() {
        return Some(github_source(owner, repo, None));
    }

    match rest {
        ["tree", reference @ ..] if !reference.is_empty() => {
            let value = decode_ref_segments(reference);
            let kind = if SHA_REGEX.is_match(&value) {
                PluginGithubRefKind::Sha
            } else {
                PluginGithubRefKind::Branch
            };
            Some(github_source(
                owner,
                repo,
                Some(PluginGithubRef { kind, value }),
            ))
        }
        ["releases", "tag", reference @ ..] if !reference.is_empty() => Some(github_source(
            owner,
            repo,
            Some(PluginGithubRef {
                kind: PluginGithubRefKind::Tag,
                value: decode_ref_segments(reference),
            }),
        )),
        ["commit", reference @ ..] if !reference.is_empty() => Some(github_source(
            owner,
            repo,
            Some(PluginGithubRef {
                kind: PluginGithubRefKind::Sha,
                value: decode_ref_segments(reference),
            }),
        )),
        _ => None,
    }
}

fn github_source(owner: &str, repo: &str, reference: Option<PluginGithubRef>) -> ResolvedSource {
    ResolvedSource::Github {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        reference,
    }
}

// Original: source.ts, decodeRefSegments().
fn decode_ref_segments(segments: &[&str]) -> String {
    segments
        .iter()
        .map(|segment| {
            percent_decode_str(segment)
                .decode_utf8()
                .map_or_else(|_| (*segment).to_owned(), |decoded| decoded.into_owned())
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_local_and_zip_sources_in_original_order() {
        assert_eq!(
            resolve_install_source("  https://example.com/plugin.zip  ").unwrap(),
            ResolvedSource::ZipUrl {
                path: "https://example.com/plugin.zip".to_owned(),
            }
        );
        let local = std::env::current_dir()
            .unwrap()
            .join("plugins")
            .join("demo");
        assert_eq!(
            resolve_install_source(&format!("  {}  ", local.display())).unwrap(),
            ResolvedSource::LocalPath {
                path: local.to_string_lossy().into_owned(),
            }
        );
        assert_eq!(
            resolve_install_source(" relative/plugin ")
                .unwrap_err()
                .to_string(),
            "Plugin root must be an absolute path (got \" relative/plugin \")"
        );
    }

    #[test]
    fn resolves_supported_github_url_forms() {
        assert_eq!(
            resolve_install_source("https://github.com/moonshot/demo.git").unwrap(),
            ResolvedSource::Github {
                owner: "moonshot".to_owned(),
                repo: "demo".to_owned(),
                reference: None,
            }
        );
        assert_eq!(
            resolve_install_source("https://www.github.com/moonshot/demo/tree/feature%2Fone/part")
                .unwrap(),
            ResolvedSource::Github {
                owner: "moonshot".to_owned(),
                repo: "demo".to_owned(),
                reference: Some(PluginGithubRef {
                    kind: PluginGithubRefKind::Branch,
                    value: "feature/one/part".to_owned(),
                }),
            }
        );
        assert_eq!(
            resolve_install_source("https://github.com/moonshot/demo/tree/0123abc").unwrap(),
            ResolvedSource::Github {
                owner: "moonshot".to_owned(),
                repo: "demo".to_owned(),
                reference: Some(PluginGithubRef {
                    kind: PluginGithubRefKind::Sha,
                    value: "0123abc".to_owned(),
                }),
            }
        );
        assert!(matches!(
            resolve_install_source("https://github.com/moonshot/demo/releases/tag/v1%2E2"),
            Ok(ResolvedSource::Github {
                reference: Some(PluginGithubRef {
                    kind: PluginGithubRefKind::Tag,
                    ref value,
                }),
                ..
            }) if value == "v1.2"
        ));
        assert!(matches!(
            resolve_install_source("https://github.com/moonshot/demo/commit/deadbee"),
            Ok(ResolvedSource::Github {
                reference: Some(PluginGithubRef {
                    kind: PluginGithubRefKind::Sha,
                    ref value,
                }),
                ..
            }) if value == "deadbee"
        ));
    }

    #[test]
    fn unsupported_github_paths_remain_generic_zip_urls() {
        assert_eq!(
            resolve_install_source("https://github.com/moonshot/demo/issues/1").unwrap(),
            ResolvedSource::ZipUrl {
                path: "https://github.com/moonshot/demo/issues/1".to_owned(),
            }
        );
        assert_eq!(
            resolve_install_source("http://github.com/moonshot/demo").unwrap(),
            ResolvedSource::ZipUrl {
                path: "http://github.com/moonshot/demo".to_owned(),
            }
        );
    }
}
