use std::{error::Error, fmt, path::PathBuf};

use super::build_info::KIMI_BUILD_INFO;

pub const CLI_USER_AGENT_PRODUCT: &str = "kimi-code-cli";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostIdentity {
    pub user_agent_product: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityError {
    field_name: &'static str,
}

impl IdentityError {
    fn new(field_name: &'static str) -> Self {
        Self { field_name }
    }
}

impl fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} must be a non-empty ASCII string.",
            self.field_name
        )
    }
}

impl Error for IdentityError {}

// Original:
//   apps/kimi-code/src/cli/version.ts
//   getHostPackageRoot()
//
// Rust adaptation:
//   Cargo provides the host package root at compile time, removing the
//   JavaScript runtime's need to walk upward looking for package.json.
pub fn get_host_package_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn get_host_manifest_path() -> PathBuf {
    get_host_package_root().join("Cargo.toml")
}

// Original:
//   apps/kimi-code/src/cli/version.ts
//   getVersion()
pub fn get_version() -> &'static str {
    KIMI_BUILD_INFO.version.unwrap_or(env!("CARGO_PKG_VERSION"))
}

// Original:
//   apps/kimi-code/src/cli/version.ts
//   createKimiCodeHostIdentity()
pub fn create_kimi_code_host_identity(version: &str) -> HostIdentity {
    HostIdentity {
        user_agent_product: CLI_USER_AGENT_PRODUCT.to_owned(),
        version: version.to_owned(),
    }
}

pub fn default_kimi_code_host_identity() -> HostIdentity {
    create_kimi_code_host_identity(get_version())
}

// Original:
//   apps/kimi-code/src/cli/version.ts
//   createKimiCodeUserAgent()
pub fn create_kimi_code_user_agent(version: &str) -> Result<String, IdentityError> {
    create_kimi_user_agent(&create_kimi_code_host_identity(version))
}

fn create_kimi_user_agent(identity: &HostIdentity) -> Result<String, IdentityError> {
    let product = required_ascii_header(&identity.user_agent_product, "Kimi identity product")?;
    let version = required_ascii_header(&identity.version, "Kimi identity version")?;
    Ok(format!("{product}/{version}"))
}

fn required_ascii_header(value: &str, field_name: &'static str) -> Result<String, IdentityError> {
    let cleaned = value
        .chars()
        .filter(|character| matches!(*character as u32, 0x20..=0x7e))
        .collect::<String>();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        Err(IdentityError::new(field_name))
    } else {
        Ok(cleaned.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_the_cargo_package_manifest_and_version() {
        let manifest = get_host_manifest_path();
        assert!(manifest.ends_with("Cargo.toml"));
        assert!(manifest.is_file());
        assert_eq!(manifest.parent(), Some(get_host_package_root().as_path()));
        assert!(!get_version().is_empty());
        assert_eq!(
            get_version(),
            KIMI_BUILD_INFO.version.unwrap_or(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn builds_the_expected_host_identity_and_user_agent() {
        assert_eq!(
            create_kimi_code_host_identity("1.2.3"),
            HostIdentity {
                user_agent_product: "kimi-code-cli".to_owned(),
                version: "1.2.3".to_owned(),
            }
        );
        assert_eq!(
            create_kimi_code_user_agent("1.2.3").expect("user agent"),
            "kimi-code-cli/1.2.3"
        );
    }

    #[test]
    fn sanitizes_ascii_headers_like_the_oauth_identity_helper() {
        assert_eq!(
            create_kimi_user_agent(&HostIdentity {
                user_agent_product: " kimi-浣犲ソcode ".to_owned(),
                version: " 1.2.3\n".to_owned(),
            })
            .expect("sanitized user agent"),
            "kimi-code/1.2.3"
        );
    }

    #[test]
    fn rejects_identity_fields_that_clean_to_empty_ascii() {
        let error = create_kimi_user_agent(&HostIdentity {
            user_agent_product: "浣犲ソ".to_owned(),
            version: "1.2.3".to_owned(),
        })
        .expect_err("invalid product");
        assert_eq!(
            error.to_string(),
            "Kimi identity product must be a non-empty ASCII string."
        );
    }
}
