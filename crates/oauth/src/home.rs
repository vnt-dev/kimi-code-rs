use std::{
    collections::HashMap,
    error::Error,
    ffi::{OsStr, OsString},
    fmt,
    path::{Path, PathBuf},
};

const KIMI_CODE_HOME_ENV: &str = "KIMI_CODE_HOME";
const KIMI_CODE_DATA_DIR_NAME: &str = ".kimi-code";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HomeDirectoryUnavailable;

impl fmt::Display for HomeDirectoryUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("could not resolve the current user's home directory")
    }
}

impl Error for HomeDirectoryUnavailable {}

// Original:
//   packages/oauth/src/toolkit.ts
//   defaultKimiHome()
pub(crate) fn default_kimi_home() -> Result<PathBuf, HomeDirectoryUnavailable> {
    let environment = std::env::vars_os().collect::<HashMap<OsString, OsString>>();
    default_kimi_home_from(&environment, dirs::home_dir().as_deref())
}

fn default_kimi_home_from(
    environment: &HashMap<OsString, OsString>,
    home_dir: Option<&Path>,
) -> Result<PathBuf, HomeDirectoryUnavailable> {
    if let Some(override_path) = environment.get(OsStr::new(KIMI_CODE_HOME_ENV))
        && !override_path.is_empty()
    {
        return Ok(PathBuf::from(override_path));
    }

    home_dir
        .map(|home_dir| home_dir.join(KIMI_CODE_DATA_DIR_NAME))
        .ok_or(HomeDirectoryUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_home_prefers_non_empty_environment_override() {
        let environment = HashMap::from([(
            OsString::from(KIMI_CODE_HOME_ENV),
            OsString::from("relative/oauth-home"),
        )]);

        assert_eq!(
            default_kimi_home_from(&environment, Some(Path::new("/home/kimi"))).unwrap(),
            PathBuf::from("relative/oauth-home")
        );
    }

    #[test]
    fn default_home_falls_back_to_user_home_and_reports_absence() {
        assert_eq!(
            default_kimi_home_from(&HashMap::new(), Some(Path::new("/home/kimi"))).unwrap(),
            PathBuf::from("/home/kimi/.kimi-code")
        );
        assert_eq!(
            default_kimi_home_from(&HashMap::new(), None),
            Err(HomeDirectoryUnavailable)
        );
    }
}
