//! GitHub plugin source resolution without the REST API.
//!
//! Original: `packages/agent-core-v2/src/app/plugin/github-resolver.ts`.

use std::sync::LazyLock;
use std::time::Duration;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use regex::Regex;
use reqwest::{Client, StatusCode, header};
use thiserror::Error;

use super::types::{PluginGithubRef, PluginGithubRefKind};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

static COMMIT_SHA_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)Grit::Commit/([0-9a-f]{40})").expect("commit SHA regex must compile")
});
static RELEASE_TAG_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"/releases/tag/([^/?#]+)").expect("release tag regex must compile")
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GithubSourceInput {
    pub owner: String,
    pub repo: String,
    pub reference: Option<PluginGithubRef>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GithubSourceResolution {
    pub tarball_url: String,
    pub display_version: String,
    pub reference: PluginGithubRef,
}

#[derive(Debug, Error)]
pub enum GithubResolverError {
    #[error("Could not resolve {owner}/{repo}@{reference}: HTTP {status} {status_text}.")]
    CommitHttp {
        owner: String,
        repo: String,
        reference: String,
        status: u16,
        status_text: String,
    },
    #[error("Could not resolve {owner}/{repo}@{reference} to a commit SHA.")]
    CommitShaMissing {
        owner: String,
        repo: String,
        reference: String,
    },
    #[error("Repository `{owner}/{repo}` not found or not accessible.")]
    RepositoryNotFound { owner: String, repo: String },
    #[error("Could not access `{owner}/{repo}`: HTTP {status} {status_text}.")]
    RepositoryAccess {
        owner: String,
        repo: String,
        status: u16,
        status_text: String,
    },
    #[error(
        "Could not look up latest release of `{owner}/{repo}`: HTTP {status} {status_text} ({url}). Pin a specific ref with `/tree/<branch|tag|sha>` to bypass release lookup."
    )]
    LatestRelease {
        owner: String,
        repo: String,
        status: u16,
        status_text: String,
        url: String,
    },
    #[error(transparent)]
    Request(#[from] reqwest::Error),
}

// Original: github-resolver.ts, resolveGithubCommitSha().
pub async fn resolve_github_commit_sha(
    owner: &str,
    repo: &str,
    reference: &str,
) -> Result<String, GithubResolverError> {
    let client = github_client()?;
    let encoded = encode_codeload_ref_path(reference);
    let url = format!("https://github.com/{owner}/{repo}/commits/{encoded}.atom");
    let response = client
        .get(url)
        .header(header::ACCEPT, "application/atom+xml")
        .header(header::RANGE, "bytes=0-4095")
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(GithubResolverError::CommitHttp {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            reference: reference.to_owned(),
            status: response.status().as_u16(),
            status_text: status_text(response.status()),
        });
    }
    let feed = response.text().await?;
    extract_commit_sha(&feed).ok_or_else(|| GithubResolverError::CommitShaMissing {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        reference: reference.to_owned(),
    })
}

// Original: github-resolver.ts, resolveGithubSource().
pub async fn resolve_github_source(
    input: &GithubSourceInput,
) -> Result<GithubSourceResolution, GithubResolverError> {
    if let Some(reference) = &input.reference {
        return Ok(resolution(&input.owner, &input.repo, reference.clone()));
    }

    let client = github_client()?;
    if let Some(tag) = try_resolve_latest_release_tag(&client, &input.owner, &input.repo).await? {
        return Ok(resolution(
            &input.owner,
            &input.repo,
            PluginGithubRef {
                kind: PluginGithubRefKind::Tag,
                value: tag,
            },
        ));
    }

    let tarball_url = format!(
        "https://codeload.github.com/{}/{}/zip/HEAD",
        input.owner, input.repo
    );
    let response = client.head(&tarball_url).send().await?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err(GithubResolverError::RepositoryNotFound {
            owner: input.owner.clone(),
            repo: input.repo.clone(),
        });
    }
    if !response.status().is_success() {
        return Err(GithubResolverError::RepositoryAccess {
            owner: input.owner.clone(),
            repo: input.repo.clone(),
            status: response.status().as_u16(),
            status_text: status_text(response.status()),
        });
    }
    Ok(GithubSourceResolution {
        tarball_url,
        display_version: "HEAD".to_owned(),
        reference: PluginGithubRef {
            kind: PluginGithubRefKind::Branch,
            value: "HEAD".to_owned(),
        },
    })
}

async fn try_resolve_latest_release_tag(
    client: &Client,
    owner: &str,
    repo: &str,
) -> Result<Option<String>, GithubResolverError> {
    let url = format!("https://github.com/{owner}/{repo}/releases/latest");
    let response = client.get(&url).send().await?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !matches!(
        response.status(),
        StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND
    ) {
        return Err(GithubResolverError::LatestRelease {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            status: response.status().as_u16(),
            status_text: status_text(response.status()),
            url,
        });
    }
    let Some(location) = response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(None);
    };
    Ok(extract_release_tag(location))
}

fn resolution(owner: &str, repo: &str, reference: PluginGithubRef) -> GithubSourceResolution {
    GithubSourceResolution {
        tarball_url: codeload_url(owner, repo, &reference),
        display_version: reference.value.clone(),
        reference,
    }
}

fn codeload_url(owner: &str, repo: &str, reference: &PluginGithubRef) -> String {
    let base = format!("https://codeload.github.com/{owner}/{repo}/zip");
    let encoded = encode_codeload_ref_path(&reference.value);
    match reference.kind {
        PluginGithubRefKind::Tag => format!("{base}/refs/tags/{encoded}"),
        PluginGithubRefKind::Branch | PluginGithubRefKind::Sha => format!("{base}/{encoded}"),
    }
}

fn encode_codeload_ref_path(value: &str) -> String {
    value
        .split('/')
        .map(|segment| utf8_percent_encode(segment, URI_COMPONENT).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

fn extract_commit_sha(feed: &str) -> Option<String> {
    COMMIT_SHA_REGEX
        .captures(feed)?
        .get(1)
        .map(|value| value.as_str().to_ascii_lowercase())
}

fn extract_release_tag(location: &str) -> Option<String> {
    let encoded = RELEASE_TAG_REGEX.captures(location)?.get(1)?.as_str();
    Some(
        percent_decode_str(encoded)
            .decode_utf8()
            .map_or_else(|_| encoded.to_owned(), |value| value.into_owned()),
    )
}

fn github_client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

fn status_text(status: StatusCode) -> String {
    status.canonical_reason().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn explicit_refs_resolve_without_network_and_preserve_codeload_rules() {
        for (kind, value, suffix) in [
            (PluginGithubRefKind::Branch, "feature/a b", "feature/a%20b"),
            (PluginGithubRefKind::Tag, "v1.0", "refs/tags/v1.0"),
            (PluginGithubRefKind::Sha, "deadbeef", "deadbeef"),
        ] {
            let resolved = resolve_github_source(&GithubSourceInput {
                owner: "moonshot".to_owned(),
                repo: "demo".to_owned(),
                reference: Some(PluginGithubRef {
                    kind,
                    value: value.to_owned(),
                }),
            })
            .await
            .unwrap();
            assert_eq!(resolved.display_version, value);
            assert_eq!(
                resolved.tarball_url,
                format!("https://codeload.github.com/moonshot/demo/zip/{suffix}")
            );
        }
    }

    #[test]
    fn extracts_atom_sha_and_decodes_latest_release_location() {
        assert_eq!(
            extract_commit_sha(
                "<id>tag:github.com,2008:Grit::Commit/ABCDEF0123456789ABCDEF0123456789ABCDEF01</id>"
            )
            .as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef01")
        );
        assert_eq!(extract_commit_sha("no commit"), None);
        assert_eq!(
            extract_release_tag("https://github.com/o/r/releases/tag/release%2Fv1?x=1").as_deref(),
            Some("release/v1")
        );
        assert_eq!(extract_release_tag("https://github.com/o/r/releases"), None);
    }
}
