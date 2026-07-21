use std::{collections::HashSet, path::Path};

use serde_json::{Map, Value};

use super::types::{
    InstallSource, UpdateInstallActive, UpdateInstallFailure, UpdateInstallState,
    UpdateInstallSuccess, empty_update_install_state,
};
use crate::utils::{
    paths::get_update_install_state_file,
    persistence::{PersistenceError, read_json_file, write_json_file},
};

// Original:
//   apps/kimi-code/src/cli/update/install-state.ts
//   readUpdateInstallState()
pub async fn read_update_install_state() -> UpdateInstallState {
    let Ok(file_path) = get_update_install_state_file() else {
        return empty_update_install_state();
    };
    read_update_install_state_from(&file_path).await
}

pub async fn read_update_install_state_from(file_path: &Path) -> UpdateInstallState {
    read_json_file(
        file_path,
        |value| {
            parse_update_install_state(value)
                .ok_or_else(|| "invalid update install state".to_owned())
        },
        empty_update_install_state(),
    )
    .await
    .unwrap_or_else(|_| empty_update_install_state())
}

// Original:
//   apps/kimi-code/src/cli/update/install-state.ts
//   writeUpdateInstallState()
pub async fn write_update_install_state(
    value: &UpdateInstallState,
) -> Result<(), PersistenceError> {
    let file_path = get_update_install_state_file()
        .map_err(|error| PersistenceError::Invalid(error.to_string()))?;
    write_update_install_state_to(value, &file_path).await
}

pub async fn write_update_install_state_to(
    value: &UpdateInstallState,
    file_path: &Path,
) -> Result<(), PersistenceError> {
    write_json_file(
        file_path,
        |serialized| {
            let parsed = parse_update_install_state(serialized)
                .ok_or_else(|| "invalid update install state".to_owned())?;
            serde_json::to_value(parsed).map_err(|error| error.to_string())
        },
        value,
    )
    .await
}

fn parse_update_install_state(value: Value) -> Option<UpdateInstallState> {
    let object = value.as_object()?;
    if !has_exact_keys(object, &["active", "lastFailure", "lastSuccess"]) {
        return None;
    }
    Some(UpdateInstallState {
        active: parse_nullable(object.get("active")?, parse_active)?,
        last_failure: parse_nullable(object.get("lastFailure")?, parse_failure)?,
        last_success: parse_nullable(object.get("lastSuccess")?, parse_success)?,
    })
}

fn parse_nullable<T>(value: &Value, parse: fn(&Value) -> Option<T>) -> Option<Option<T>> {
    if value.is_null() {
        Some(None)
    } else {
        parse(value).map(Some)
    }
}

fn parse_active(value: &Value) -> Option<UpdateInstallActive> {
    let object = value.as_object()?;
    if !has_exact_keys(object, &["version", "source", "startedAt"]) {
        return None;
    }
    Some(UpdateInstallActive {
        version: nonempty_string(object.get("version")?)?,
        source: parse_source(object.get("source")?.as_str()?)?,
        started_at: nonempty_string(object.get("startedAt")?)?,
    })
}

fn parse_failure(value: &Value) -> Option<UpdateInstallFailure> {
    let object = value.as_object()?;
    if !has_exact_keys(object, &["version", "failedAt", "attempts"]) {
        return None;
    }
    let attempts = object.get("attempts")?.as_u64()?;
    if attempts == 0 {
        return None;
    }
    Some(UpdateInstallFailure {
        version: nonempty_string(object.get("version")?)?,
        failed_at: nonempty_string(object.get("failedAt")?)?,
        attempts,
    })
}

fn parse_success(value: &Value) -> Option<UpdateInstallSuccess> {
    let object = value.as_object()?;
    if !has_exact_keys(object, &["version", "installedAt", "notifiedAt"]) {
        return None;
    }
    Some(UpdateInstallSuccess {
        version: nonempty_string(object.get("version")?)?,
        installed_at: nonempty_string(object.get("installedAt")?)?,
        notified_at: nullable_nonempty_string(object.get("notifiedAt")?)?,
    })
}

fn parse_source(value: &str) -> Option<InstallSource> {
    match value {
        "npm-global" => Some(InstallSource::NpmGlobal),
        "pnpm-global" => Some(InstallSource::PnpmGlobal),
        "yarn-global" => Some(InstallSource::YarnGlobal),
        "bun-global" => Some(InstallSource::BunGlobal),
        "homebrew" => Some(InstallSource::Homebrew),
        "native" => Some(InstallSource::Native),
        "unsupported" => Some(InstallSource::Unsupported),
        _ => None,
    }
}

fn nonempty_string(value: &Value) -> Option<String> {
    let value = value.as_str()?;
    (!value.is_empty()).then(|| value.to_owned())
}

fn nullable_nonempty_string(value: &Value) -> Option<Option<String>> {
    if value.is_null() {
        Some(None)
    } else {
        nonempty_string(value).map(Some)
    }
}

fn has_exact_keys(object: &Map<String, Value>, keys: &[&str]) -> bool {
    let expected = keys.iter().copied().collect::<HashSet<_>>();
    object.len() == expected.len() && object.keys().all(|key| expected.contains(key.as_str()))
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_file() -> PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!("kimi-install-state-{}-{id}", std::process::id()))
            .join("updates")
            .join("install.json")
    }

    fn populated_state() -> UpdateInstallState {
        UpdateInstallState {
            active: Some(UpdateInstallActive {
                version: "0.5.0".to_owned(),
                source: InstallSource::NpmGlobal,
                started_at: "2026-04-23T08:00:00.000Z".to_owned(),
            }),
            last_failure: Some(UpdateInstallFailure {
                version: "0.4.0".to_owned(),
                failed_at: "2026-04-22T08:00:00.000Z".to_owned(),
                attempts: 1,
            }),
            last_success: Some(UpdateInstallSuccess {
                version: "0.3.0".to_owned(),
                installed_at: "2026-04-21T08:00:00.000Z".to_owned(),
                notified_at: None,
            }),
        }
    }

    async fn cleanup(file: &Path) {
        if let Some(root) = file.parent().and_then(Path::parent) {
            let _ = tokio::fs::remove_dir_all(root).await;
        }
    }

    #[tokio::test]
    async fn missing_and_corrupt_files_return_empty_state() {
        let file = temp_file();
        assert_eq!(
            read_update_install_state_from(&file).await,
            empty_update_install_state()
        );
        tokio::fs::create_dir_all(file.parent().expect("parent"))
            .await
            .expect("create parent");
        tokio::fs::write(&file, "{\"broken\"")
            .await
            .expect("write corrupt");
        assert_eq!(
            read_update_install_state_from(&file).await,
            empty_update_install_state()
        );
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn writes_overwrites_and_reads_the_strict_state_shape() {
        let file = temp_file();
        write_update_install_state_to(&empty_update_install_state(), &file)
            .await
            .expect("initial state");
        let state = populated_state();
        write_update_install_state_to(&state, &file)
            .await
            .expect("populated state");
        assert_eq!(read_update_install_state_from(&file).await, state);
        cleanup(&file).await;
    }

    #[tokio::test]
    async fn rejects_unknown_fields_zero_attempts_and_empty_strings() {
        let file = temp_file();
        tokio::fs::create_dir_all(file.parent().expect("parent"))
            .await
            .expect("create parent");
        for invalid in [
            serde_json::json!({
                "active": null, "lastFailure": null, "lastSuccess": null, "extra": true
            }),
            serde_json::json!({
                "active": null,
                "lastFailure": { "version": "0.4.0", "failedAt": "now", "attempts": 0 },
                "lastSuccess": null
            }),
            serde_json::json!({
                "active": { "version": "", "source": "native", "startedAt": "now" },
                "lastFailure": null,
                "lastSuccess": null
            }),
        ] {
            tokio::fs::write(&file, serde_json::to_vec(&invalid).expect("invalid json"))
                .await
                .expect("write invalid");
            assert_eq!(
                read_update_install_state_from(&file).await,
                empty_update_install_state()
            );
        }
        cleanup(&file).await;
    }
}
