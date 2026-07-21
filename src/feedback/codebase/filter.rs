use std::path::Path;

pub const DEFAULT_MAX_FILES: usize = 50_000;
pub const DEFAULT_MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;
pub const DEFAULT_MAX_ARCHIVE_SIZE: u64 = 500 * 1024 * 1024;

const IGNORED_DIR_NAMES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".turbo",
    ".cache",
    ".parcel-cache",
    "coverage",
    ".nyc_output",
    "target",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".venv",
    "venv",
    "env",
    ".idea",
];

const SENSITIVE_DIR_NAMES: &[&str] = &[".ssh", ".gnupg", ".aws", ".kube", ".docker"];
const SENSITIVE_FILE_NAMES: &[&str] = &[
    ".env",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "credentials.json",
    "service-account.json",
    "serviceAccount.json",
    ".netrc",
    ".htpasswd",
    ".pypirc",
    ".npmrc",
    ".envrc",
    ".yarnrc",
    ".yarnrc.yml",
];
const SENSITIVE_FILE_SUFFIXES: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".jks", ".keystore"];
const ENV_FILE_ALLOWED_SUFFIXES: &[&str] = &[".example", ".sample", ".template"];

// Original: `src/feedback/codebase/filter.ts`, `isIgnoredDirName()`.
pub fn is_ignored_dir_name(name: &str) -> bool {
    IGNORED_DIR_NAMES.contains(&name)
}

// Original: `isSensitivePath()`.
pub fn is_sensitive_path(relative_path: &str) -> bool {
    let normalized = relative_path.replace('\\', "/");
    let mut segments = normalized.split('/').collect::<Vec<_>>();
    let Some(base) = segments.pop().filter(|base| !base.is_empty()) else {
        return false;
    };
    if segments
        .iter()
        .any(|segment| SENSITIVE_DIR_NAMES.contains(segment))
    {
        return true;
    }
    if SENSITIVE_FILE_NAMES.contains(&base)
        || SENSITIVE_FILE_SUFFIXES
            .iter()
            .any(|suffix| base.ends_with(suffix))
    {
        return true;
    }
    if let Some(suffix) = base.strip_prefix(".env.") {
        let dotted = format!(".{suffix}");
        return !ENV_FILE_ALLOWED_SUFFIXES.contains(&dotted.as_str());
    }
    false
}

pub fn is_sensitive_file(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .ok()
        .is_some_and(|relative| is_sensitive_path(&relative.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_source_control_dependencies_build_outputs_and_caches() {
        for name in [
            ".git",
            "node_modules",
            "dist",
            "target",
            "__pycache__",
            ".idea",
        ] {
            assert!(is_ignored_dir_name(name), "{name}");
        }
        for name in ["src", "tests", "assets", "node_modules-old"] {
            assert!(!is_ignored_dir_name(name), "{name}");
        }
    }

    #[test]
    fn rejects_sensitive_directories_at_any_parent_depth() {
        for path in [
            ".ssh/config",
            "home/.aws/credentials",
            "repo/.kube/config",
            "nested/.docker/config.json",
        ] {
            assert!(is_sensitive_path(path), "{path}");
        }
        assert!(!is_sensitive_path("src/.aws_client.rs"));
    }

    #[test]
    fn rejects_credentials_keys_and_private_archive_formats() {
        for path in [
            ".env",
            "config/id_ed25519",
            "credentials.json",
            "tls/server.pem",
            "tls/client.key",
            "signing/release.jks",
            ".npmrc",
        ] {
            assert!(is_sensitive_path(path), "{path}");
        }
    }

    #[test]
    fn permits_only_documented_env_templates() {
        for path in [".env.example", ".env.sample", ".env.template"] {
            assert!(!is_sensitive_path(path), "{path}");
        }
        for path in [".env.local", ".env.production", ".env.example.bak"] {
            assert!(is_sensitive_path(path), "{path}");
        }
    }

    #[test]
    fn accepts_windows_separators_and_root_relative_paths() {
        assert!(is_sensitive_path("nested\\.ssh\\config"));
        assert!(is_sensitive_file(
            Path::new("C:/repo/.env"),
            Path::new("C:/repo")
        ));
        assert!(!is_sensitive_file(
            Path::new("C:/other/.env"),
            Path::new("C:/repo")
        ));
    }
}
