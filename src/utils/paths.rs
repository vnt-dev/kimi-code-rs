use std::{
    collections::HashMap,
    error::Error,
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
};

use md5::{Digest, Md5};

pub const KIMI_CODE_HOME_ENV: &str = "KIMI_CODE_HOME";
pub const KIMI_CODE_DATA_DIR_NAME: &str = ".kimi-code";
pub const KIMI_CODE_LOG_DIR_NAME: &str = "logs";
pub const KIMI_CODE_CACHE_DIR_NAME: &str = "cache";
pub const KIMI_CODE_UPDATE_DIR_NAME: &str = "updates";
pub const KIMI_CODE_BIN_DIR_NAME: &str = "bin";
pub const KIMI_CODE_UPDATE_STATE_FILE_NAME: &str = "latest.json";
pub const KIMI_CODE_UPDATE_INSTALL_STATE_FILE_NAME: &str = "install.json";
pub const KIMI_CODE_UPDATE_INSTALL_LOCK_FILE_NAME: &str = "install.lock";
pub const KIMI_CODE_UPDATE_ROLLOUT_LOG_FILE_NAME: &str = "rollout.log";
pub const KIMI_CODE_INPUT_HISTORY_DIR_NAME: &str = "user-history";
pub const KIMI_CODE_BANNER_DIR_NAME: &str = "banner";
pub const KIMI_CODE_BANNER_STATE_FILE_NAME: &str = "state.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HomeDirectoryUnavailable;

impl fmt::Display for HomeDirectoryUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("could not resolve the current user's home directory")
    }
}

impl Error for HomeDirectoryUnavailable {}

// Original:
//   apps/kimi-code/src/utils/paths.ts
//   getDataDir()
pub fn get_data_dir() -> Result<PathBuf, HomeDirectoryUnavailable> {
    let environment = std::env::vars_os().collect::<HashMap<OsString, OsString>>();
    get_data_dir_from(&environment, dirs::home_dir().as_deref())
}

pub fn get_data_dir_from(
    environment: &HashMap<OsString, OsString>,
    home_dir: Option<&Path>,
) -> Result<PathBuf, HomeDirectoryUnavailable> {
    if let Some(data_dir) = environment.get(&OsString::from(KIMI_CODE_HOME_ENV))
        && !data_dir.is_empty()
    {
        return Ok(PathBuf::from(data_dir));
    }
    home_dir
        .map(|home_dir| home_dir.join(KIMI_CODE_DATA_DIR_NAME))
        .ok_or(HomeDirectoryUnavailable)
}

pub fn get_log_dir() -> Result<PathBuf, HomeDirectoryUnavailable> {
    Ok(get_log_dir_from(&get_data_dir()?))
}

pub fn get_cache_dir() -> Result<PathBuf, HomeDirectoryUnavailable> {
    Ok(get_cache_dir_from(&get_data_dir()?))
}

pub fn get_bin_dir() -> Result<PathBuf, HomeDirectoryUnavailable> {
    Ok(get_bin_dir_from(&get_data_dir()?))
}

pub fn get_update_state_file() -> Result<PathBuf, HomeDirectoryUnavailable> {
    Ok(get_update_state_file_from(&get_data_dir()?))
}

pub fn get_update_install_state_file() -> Result<PathBuf, HomeDirectoryUnavailable> {
    Ok(get_update_install_state_file_from(&get_data_dir()?))
}

pub fn get_update_install_lock_file() -> Result<PathBuf, HomeDirectoryUnavailable> {
    Ok(get_update_install_lock_file_from(&get_data_dir()?))
}

pub fn get_update_rollout_log_file() -> Result<PathBuf, HomeDirectoryUnavailable> {
    Ok(get_update_rollout_log_file_from(&get_data_dir()?))
}

pub fn get_banner_state_file() -> Result<PathBuf, HomeDirectoryUnavailable> {
    Ok(get_banner_state_file_from(&get_data_dir()?))
}

pub fn get_input_history_file(work_dir: &str) -> Result<PathBuf, HomeDirectoryUnavailable> {
    Ok(get_input_history_file_from(&get_data_dir()?, work_dir))
}

pub fn get_log_dir_from(data_dir: &Path) -> PathBuf {
    data_dir.join(KIMI_CODE_LOG_DIR_NAME)
}

pub fn get_cache_dir_from(data_dir: &Path) -> PathBuf {
    data_dir.join(KIMI_CODE_CACHE_DIR_NAME)
}

pub fn get_bin_dir_from(data_dir: &Path) -> PathBuf {
    data_dir.join(KIMI_CODE_BIN_DIR_NAME)
}

pub fn get_update_state_file_from(data_dir: &Path) -> PathBuf {
    data_dir
        .join(KIMI_CODE_UPDATE_DIR_NAME)
        .join(KIMI_CODE_UPDATE_STATE_FILE_NAME)
}

pub fn get_update_install_state_file_from(data_dir: &Path) -> PathBuf {
    data_dir
        .join(KIMI_CODE_UPDATE_DIR_NAME)
        .join(KIMI_CODE_UPDATE_INSTALL_STATE_FILE_NAME)
}

pub fn get_update_install_lock_file_from(data_dir: &Path) -> PathBuf {
    data_dir
        .join(KIMI_CODE_UPDATE_DIR_NAME)
        .join(KIMI_CODE_UPDATE_INSTALL_LOCK_FILE_NAME)
}

pub fn get_update_rollout_log_file_from(data_dir: &Path) -> PathBuf {
    data_dir
        .join(KIMI_CODE_UPDATE_DIR_NAME)
        .join(KIMI_CODE_UPDATE_ROLLOUT_LOG_FILE_NAME)
}

pub fn get_banner_state_file_from(data_dir: &Path) -> PathBuf {
    get_cache_dir_from(data_dir)
        .join(KIMI_CODE_BANNER_DIR_NAME)
        .join(KIMI_CODE_BANNER_STATE_FILE_NAME)
}

pub fn get_input_history_file_from(data_dir: &Path, work_dir: &str) -> PathBuf {
    let digest = Md5::digest(work_dir.as_bytes());
    let mut hash = String::with_capacity(digest.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        hash.push(char::from(HEX[usize::from(byte >> 4)]));
        hash.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    data_dir
        .join(KIMI_CODE_INPUT_HISTORY_DIR_NAME)
        .join(format!("{hash}.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(value: Option<&str>) -> HashMap<OsString, OsString> {
        value
            .map(|value| {
                HashMap::from([(OsString::from(KIMI_CODE_HOME_ENV), OsString::from(value))])
            })
            .unwrap_or_default()
    }

    #[test]
    fn data_dir_prefers_even_a_relative_environment_path() {
        let home = Path::new("/home/kimi");
        assert_eq!(
            get_data_dir_from(&environment(Some("relative/path")), Some(home))
                .expect("environment data dir"),
            PathBuf::from("relative/path")
        );
        assert_eq!(
            get_data_dir_from(&environment(Some("")), Some(home)).expect("home data dir"),
            home.join(".kimi-code")
        );
    }

    #[test]
    fn data_dir_reports_an_unavailable_home_instead_of_inventing_a_path() {
        assert_eq!(
            get_data_dir_from(&HashMap::new(), None),
            Err(HomeDirectoryUnavailable)
        );
    }

    #[test]
    fn derives_all_data_subpaths_from_the_same_root() {
        let root = Path::new("/data");
        assert_eq!(get_log_dir_from(root), PathBuf::from("/data/logs"));
        assert_eq!(get_cache_dir_from(root), PathBuf::from("/data/cache"));
        assert_eq!(get_bin_dir_from(root), PathBuf::from("/data/bin"));
        assert_eq!(
            get_update_state_file_from(root),
            PathBuf::from("/data/updates/latest.json")
        );
        assert_eq!(
            get_update_install_state_file_from(root),
            PathBuf::from("/data/updates/install.json")
        );
        assert_eq!(
            get_update_install_lock_file_from(root),
            PathBuf::from("/data/updates/install.lock")
        );
        assert_eq!(
            get_update_rollout_log_file_from(root),
            PathBuf::from("/data/updates/rollout.log")
        );
        assert_eq!(
            get_banner_state_file_from(root),
            PathBuf::from("/data/cache/banner/state.json")
        );
    }

    #[test]
    fn input_history_hash_matches_the_original_node_md5_vector() {
        assert_eq!(
            get_input_history_file_from(Path::new("/data"), "/home/user/project"),
            PathBuf::from("/data/user-history/90722f2638004be06d790eaac9ac1f8a.jsonl")
        );
    }
}
