use std::io;
use std::path::{Component, Path, PathBuf};

use axum::extract::State;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use tokio::fs;

use super::state::AppState;

pub async fn validate_web_assets(assets_dir: &Path) -> io::Result<()> {
    let metadata = fs::metadata(assets_dir.join("index.html")).await?;
    if metadata.is_file() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "index.html is not a file",
        ))
    }
}

// Original: routes/webAssets.ts, serveWebAsset().
pub async fn serve_web_asset(
    State(state): State<std::sync::Arc<AppState>>,
    method: Method,
    uri: Uri,
) -> Response {
    if method != Method::GET {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(assets_dir) = state.web_assets_dir.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if is_reserved_path(uri.path()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(file_path) = resolve_static_file(assets_dir, uri.path()).await else {
        return not_found();
    };
    let Ok(bytes) = fs::read(&file_path).await else {
        return not_found();
    };
    let content_length = bytes.len();
    let mut response = bytes.into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static(mime_type(&file_path)),
    );
    if let Ok(length) = HeaderValue::from_str(&content_length.to_string()) {
        response.headers_mut().insert(CONTENT_LENGTH, length);
    }
    response
}

async fn resolve_static_file(assets_dir: &Path, pathname: &str) -> Option<PathBuf> {
    let decoded = percent_decode(pathname)?;
    let relative = normalized_relative_path(&decoded);
    let root = absolute_path(assets_dir).ok()?;
    let requested = if relative.as_os_str().is_empty() {
        root.join("index.html")
    } else {
        root.join(&relative)
    };
    let candidate = if decoded.ends_with(['/', '\\']) {
        requested.join("index.html")
    } else {
        requested
    };
    if !candidate.starts_with(&root) {
        return None;
    }
    if fs::metadata(&candidate)
        .await
        .is_ok_and(|metadata| metadata.is_file())
    {
        return Some(candidate);
    }
    if Path::new(pathname).extension().is_some() {
        return None;
    }
    Some(root.join("index.html"))
}

fn normalized_relative_path(pathname: &str) -> PathBuf {
    let mut relative = PathBuf::new();
    for component in Path::new(pathname.trim_start_matches(['/', '\\'])).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::ParentDir => {
                relative.pop();
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    relative
}

fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex(*bytes.get(index + 1)?)?;
            let low = hex(*bytes.get(index + 2)?)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_reserved_path(pathname: &str) -> bool {
    pathname == "/api"
        || pathname.starts_with("/api/")
        || pathname == "/documentation"
        || pathname.starts_with("/documentation/")
}

fn mime_type(file_path: &Path) -> &'static str {
    match file_path
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(CONTENT_TYPE, "text/plain; charset=utf-8")],
        "Not found",
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_paths_without_escaping_the_asset_root() {
        assert_eq!(
            normalized_relative_path("/../../assets/app.js"),
            PathBuf::from("assets").join("app.js")
        );
        assert_eq!(normalized_relative_path("/"), PathBuf::new());
        assert!(percent_decode("/bad%zz").is_none());
    }

    #[test]
    fn preserves_typescript_mime_table() {
        assert_eq!(
            mime_type(Path::new("index.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            mime_type(Path::new("app.mjs")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(mime_type(Path::new("font.woff2")), "font/woff2");
        assert_eq!(mime_type(Path::new("data.bin")), "application/octet-stream");
    }
}
