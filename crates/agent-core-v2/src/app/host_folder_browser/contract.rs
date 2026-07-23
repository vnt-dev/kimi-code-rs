//! Host folder browser protocol, errors, and service contract.
//!
//! Original: `packages/agent-core-v2/src/app/hostFolderBrowser/hostFolderBrowser.ts`.

use std::{error::Error, fmt, ops::Deref, sync::Arc};

use async_trait::async_trait;

pub use kimi_code_protocol::{FsBrowseEntry, FsBrowseQuery, FsBrowseResponse, FsHomeResponse};

use crate::_base::di::instantiation::ServiceIdentifier;

pub const RECENT_ROOTS_LIMIT: usize = 8;

macro_rules! host_folder_error {
    ($name:ident, $message:literal) => {
        #[derive(Clone, Debug, Eq, PartialEq)]
        pub struct $name {
            pub path: String,
        }

        impl $name {
            pub fn new(path: impl Into<String>) -> Self {
                Self { path: path.into() }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, concat!($message, ": {}"), self.path)
            }
        }

        impl Error for $name {}
    };
}

host_folder_error!(HostFolderNotAbsoluteError, "path must be absolute");
host_folder_error!(HostFolderNotFoundError, "path not found");
host_folder_error!(HostFolderPermissionError, "permission denied");

pub type HostFolderBrowserError = Box<dyn Error + Send + Sync>;
pub type HostFolderBrowserResult<T> = Result<T, HostFolderBrowserError>;

#[async_trait]
pub trait HostFolderBrowserContract: Send + Sync {
    async fn browse(
        &self,
        absolute_path: Option<&str>,
    ) -> HostFolderBrowserResult<FsBrowseResponse>;
    async fn home(&self) -> HostFolderBrowserResult<FsHomeResponse>;
}

#[derive(Clone)]
pub struct HostFolderBrowserHandle(pub Arc<dyn HostFolderBrowserContract>);

impl Deref for HostFolderBrowserHandle {
    type Target = dyn HostFolderBrowserContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const FS_HOST_FOLDER_BROWSER_ID: ServiceIdentifier<HostFolderBrowserHandle> =
    ServiceIdentifier::new("hostFolderBrowser");

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn wire_shapes_keep_snake_case_and_nullable_parent() {
        let response = FsBrowseResponse {
            path: "/repo".into(),
            parent: None,
            entries: vec![FsBrowseEntry {
                name: "src".into(),
                path: "/repo/src".into(),
                is_dir: true,
            }],
        };
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "path": "/repo",
                "parent": null,
                "entries": [{"name": "src", "path": "/repo/src", "is_dir": true}]
            })
        );
        assert_eq!(
            serde_json::to_value(FsBrowseQuery::default()).unwrap(),
            json!({})
        );
        assert!(
            serde_json::from_value::<FsBrowseEntry>(json!({
                "name": "src", "path": "/repo/src", "is_dir": false
            }))
            .is_err()
        );
        assert_eq!(RECENT_ROOTS_LIMIT, 8);
        assert_eq!(FS_HOST_FOLDER_BROWSER_ID.to_string(), "hostFolderBrowser");
    }

    #[test]
    fn domain_errors_preserve_namesake_messages_and_paths() {
        let absolute = HostFolderNotAbsoluteError::new("relative");
        assert_eq!(absolute.path, "relative");
        assert_eq!(absolute.to_string(), "path must be absolute: relative");
        assert_eq!(
            HostFolderNotFoundError::new("/missing").to_string(),
            "path not found: /missing"
        );
        assert_eq!(
            HostFolderPermissionError::new("/private").to_string(),
            "permission denied: /private"
        );
    }
}
