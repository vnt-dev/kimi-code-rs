use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

use async_trait::async_trait;

pub const KIMI_PLUGIN_ROOT_ENV: &str = "KIMI_PLUGIN_ROOT";

#[derive(Debug)]
pub struct PluginNodeError(Box<dyn Error + Send + Sync>);

impl PluginNodeError {
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }

    fn message(message: impl Into<String>) -> Self {
        Self(Box::new(PluginNodeMessage(message.into())))
    }
}

impl fmt::Display for PluginNodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for PluginNodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

#[derive(Debug)]
struct PluginNodeMessage(String);

impl fmt::Display for PluginNodeMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for PluginNodeMessage {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginNodeInvocation {
    pub argv: Vec<String>,
    pub entry: PathBuf,
}

#[async_trait]
pub trait PluginNodeRuntime: Send + Sync {
    fn plugin_root(&self) -> Option<String>;

    fn executable_path(&self) -> String;

    async fn real_path(&self, path: &Path) -> Result<PathBuf, PluginNodeError>;

    async fn import_entry(&self, invocation: PluginNodeInvocation) -> Result<(), PluginNodeError>;
}

// Original:
//   apps/kimi-code/src/cli/sub/plugin-run-node.ts
//   runPluginNodeEntry()
//
// Rust adaptation:
//   The runtime adapter owns the JavaScript module host. The validation layer
//   still canonicalizes root and entry concurrently, rewrites argv exactly,
//   and refuses symlink/path escapes before evaluating plugin code.
pub async fn run_plugin_node_entry(
    runtime: &dyn PluginNodeRuntime,
    entry: &Path,
    args: &[String],
) -> Result<(), PluginNodeError> {
    let plugin_root = runtime
        .plugin_root()
        .filter(|root| !root.trim().is_empty())
        .ok_or_else(|| {
            PluginNodeError::message("KIMI_PLUGIN_ROOT is required to run a plugin node entry.")
        })?;

    let (root_real, entry_real) = tokio::join!(
        runtime.real_path(Path::new(&plugin_root)),
        runtime.real_path(entry)
    );
    let root_real = root_real?;
    let entry_real = entry_real?;
    if !is_within(&entry_real, &root_real) {
        return Err(PluginNodeError::message(format!(
            "Plugin node entry must be inside KIMI_PLUGIN_ROOT: {}",
            entry.display()
        )));
    }

    let mut argv = Vec::with_capacity(args.len() + 2);
    argv.push(runtime.executable_path());
    argv.push(entry_real.to_string_lossy().into_owned());
    argv.extend(args.iter().cloned());
    runtime
        .import_entry(PluginNodeInvocation {
            argv,
            entry: entry_real,
        })
        .await
}

// Original: isWithin()
pub fn is_within(candidate: &Path, root: &Path) -> bool {
    candidate == root
        || candidate.strip_prefix(root).is_ok_and(|relative| {
            !relative.as_os_str().is_empty()
                && !relative.is_absolute()
                && !relative
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
        })
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use super::*;

    struct RuntimeMock {
        root: Option<String>,
        real_paths: HashMap<PathBuf, PathBuf>,
        imports: Mutex<Vec<PluginNodeInvocation>>,
    }

    #[async_trait]
    impl PluginNodeRuntime for RuntimeMock {
        fn plugin_root(&self) -> Option<String> {
            self.root.clone()
        }

        fn executable_path(&self) -> String {
            "/opt/kimi/bin/kimi".to_owned()
        }

        async fn real_path(&self, path: &Path) -> Result<PathBuf, PluginNodeError> {
            self.real_paths
                .get(path)
                .cloned()
                .ok_or_else(|| PluginNodeError::message(format!("missing: {}", path.display())))
        }

        async fn import_entry(
            &self,
            invocation: PluginNodeInvocation,
        ) -> Result<(), PluginNodeError> {
            self.imports.lock().expect("imports").push(invocation);
            Ok(())
        }
    }

    fn runtime(entry_real: &str) -> RuntimeMock {
        RuntimeMock {
            root: Some("/configured/plugin".to_owned()),
            real_paths: HashMap::from([
                (
                    PathBuf::from("/configured/plugin"),
                    PathBuf::from("/real/plugin"),
                ),
                (PathBuf::from("entry.mjs"), PathBuf::from(entry_real)),
            ]),
            imports: Mutex::new(Vec::new()),
        }
    }

    #[tokio::test]
    async fn canonicalizes_validates_rewrites_argv_and_imports() {
        let runtime = runtime("/real/plugin/dist/tool.mjs");
        run_plugin_node_entry(
            &runtime,
            Path::new("entry.mjs"),
            &["--flag".to_owned(), "value".to_owned()],
        )
        .await
        .expect("plugin");

        assert_eq!(
            runtime.imports.lock().expect("imports").as_slice(),
            [PluginNodeInvocation {
                argv: vec![
                    "/opt/kimi/bin/kimi".to_owned(),
                    "/real/plugin/dist/tool.mjs".to_owned(),
                    "--flag".to_owned(),
                    "value".to_owned(),
                ],
                entry: PathBuf::from("/real/plugin/dist/tool.mjs"),
            }]
        );
    }

    #[tokio::test]
    async fn requires_a_non_blank_plugin_root() {
        for root in [None, Some("   ".to_owned())] {
            let runtime = RuntimeMock {
                root,
                real_paths: HashMap::new(),
                imports: Mutex::new(Vec::new()),
            };
            let error = run_plugin_node_entry(&runtime, Path::new("entry.mjs"), &[])
                .await
                .expect_err("missing root");
            assert!(error.to_string().contains("KIMI_PLUGIN_ROOT is required"));
        }
    }

    #[tokio::test]
    async fn rejects_canonical_entry_outside_the_root() {
        let runtime = runtime("/real/plugin-sibling/tool.mjs");
        let error = run_plugin_node_entry(&runtime, Path::new("entry.mjs"), &[])
            .await
            .expect_err("escape");
        assert!(
            error
                .to_string()
                .contains("must be inside KIMI_PLUGIN_ROOT")
        );
        assert!(runtime.imports.lock().expect("imports").is_empty());
    }

    #[test]
    fn containment_accepts_root_and_descendants_but_not_prefix_siblings() {
        assert!(is_within(
            Path::new("/plugins/demo"),
            Path::new("/plugins/demo")
        ));
        assert!(is_within(
            Path::new("/plugins/demo/dist/tool.mjs"),
            Path::new("/plugins/demo")
        ));
        assert!(!is_within(
            Path::new("/plugins/demo-other/tool.mjs"),
            Path::new("/plugins/demo")
        ));
        assert!(!is_within(
            Path::new("/plugins/outside"),
            Path::new("/plugins/demo")
        ));
    }
}
