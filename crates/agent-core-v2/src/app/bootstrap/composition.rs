//! Application-scope composition root.
//!
//! Original: `packages/agent-core-v2/src/app/bootstrap/bootstrap.ts`.

use std::sync::Arc;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{Scope, ScopeOptions},
            service_collection::ServiceCollection,
        },
        log::LOG_SERVICE_ID,
    },
    app::skill_catalog::{
        FileSkillDiscovery, SKILL_DISCOVERY_SERVICE_ID, SkillDiscoveryContract,
        SkillDiscoveryHandle,
    },
    persistence::{
        backends::node_fs::file_storage_service::FileStorageService,
        interface::storage::{
            FILE_SYSTEM_STORAGE_SERVICE_ID, FileSystemStorageService,
            FileSystemStorageServiceHandle,
        },
    },
};

use super::service::bootstrap_service_descriptor;
use super::{
    BOOTSTRAP_OPTIONS_ID, BOOTSTRAP_SERVICE_ID, BootstrapInput, BootstrapOptions,
    BootstrapResolveError, resolve_bootstrap_options,
};

pub struct BootstrapResult {
    pub app: Scope,
}

/// Builds the seed contributed by `bootstrapSeed()`.
pub fn bootstrap_seed(input: BootstrapInput) -> Result<ServiceCollection, BootstrapResolveError> {
    let options = resolve_bootstrap_options(input)?;
    Ok(options_seed(options))
}

/// Resolves the frozen startup snapshot and creates the root application scope.
pub fn bootstrap(input: BootstrapInput) -> Result<BootstrapResult, BootstrapResolveError> {
    bootstrap_with_extra(input, ServiceCollection::new())
}

/// Resolves the frozen startup snapshot and creates the root application scope
/// with caller-provided service overrides.
///
/// The insertion order matches the TypeScript composition root: bootstrap
/// defaults are installed first and caller-provided entries override them.
pub fn bootstrap_with_extra(
    input: BootstrapInput,
    extra: ServiceCollection,
) -> Result<BootstrapResult, BootstrapResolveError> {
    let options = resolve_bootstrap_options(input)?;
    let mut seed = ServiceCollection::new();
    seed.set_descriptor(BOOTSTRAP_SERVICE_ID, bootstrap_service_descriptor());
    append(&mut seed, options_seed(options.clone()));
    append(&mut seed, storage_seed(&options));
    append(&mut seed, skill_seed());
    append(&mut seed, extra);

    Ok(BootstrapResult {
        app: Scope::create_app(ScopeOptions {
            id: None,
            extra: seed,
        }),
    })
}

fn options_seed(options: BootstrapOptions) -> ServiceCollection {
    let mut seed = ServiceCollection::new();
    seed.set_instance(BOOTSTRAP_OPTIONS_ID, Arc::new(options));
    seed
}

fn storage_seed(options: &BootstrapOptions) -> ServiceCollection {
    let home_dir = options.home_dir.clone();
    let mut seed = ServiceCollection::new();
    seed.set_descriptor(
        FILE_SYSTEM_STORAGE_SERVICE_ID,
        SyncDescriptor::new(move |_| {
            let service: Arc<dyn FileSystemStorageService> = Arc::new(FileStorageService::new(
                home_dir.clone(),
                Some(0o700),
                Some(0o600),
            ));
            Ok(FileSystemStorageServiceHandle(service))
        })
        .delayed(),
    );
    seed
}

fn skill_seed() -> ServiceCollection {
    let mut seed = ServiceCollection::new();
    seed.set_descriptor(
        SKILL_DISCOVERY_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let log = accessor.get(LOG_SERVICE_ID)?;
            let service: Arc<dyn SkillDiscoveryContract> =
                Arc::new(FileSkillDiscovery::new((*log).clone()));
            Ok(SkillDiscoveryHandle(service))
        })
        .delayed(),
    );
    seed
}

fn append(target: &mut ServiceCollection, source: ServiceCollection) {
    for (id, entry) in source.iter() {
        target.set_erased(id, entry.clone());
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::Path};

    use crate::{
        _base::{
            di::lifecycle::Disposable,
            log::{
                AppLogService, LOG_SERVICE_ID, LogService, LogServiceHandle, resolve_logging_config,
            },
        },
        app::skill_catalog::SKILL_DISCOVERY_SERVICE_ID,
        persistence::interface::storage::{FILE_SYSTEM_STORAGE_SERVICE_ID, StorageWriteOptions},
    };

    use super::*;

    fn input(home_dir: &Path) -> BootstrapInput {
        BootstrapInput {
            home_dir: Some(home_dir.into()),
            os_home_dir: Some(home_dir.into()),
            cwd: Some(home_dir.into()),
            env: Some(HashMap::new()),
            ..BootstrapInput::default()
        }
    }

    fn log_seed(home_dir: &Path) -> ServiceCollection {
        let config = resolve_logging_config(home_dir, &HashMap::new());
        let log: Arc<dyn LogService> = Arc::new(AppLogService::new(&config));
        let mut seed = ServiceCollection::new();
        seed.set_instance(LOG_SERVICE_ID, Arc::new(LogServiceHandle(log)));
        seed
    }

    #[test]
    fn bootstrap_seed_contains_the_frozen_options() {
        let home = Path::new("/tmp/kimi-home");
        let seed = bootstrap_seed(input(home)).unwrap();
        let options = seed.get(BOOTSTRAP_OPTIONS_ID).unwrap().unwrap();
        assert_eq!(options.home_dir, home);
        assert_eq!(options.config_path, home.join("config.toml"));
    }

    #[test]
    fn bootstrap_uses_an_empty_extra_seed_by_default() {
        let home = Path::new("/tmp/kimi-home");
        let result = bootstrap(input(home)).unwrap();
        assert_eq!(result.app.id(), "app");
        result.app.get(FILE_SYSTEM_STORAGE_SERVICE_ID).unwrap();
        result.app.dispose().unwrap();
    }

    #[tokio::test]
    async fn bootstrap_seeds_bootstrap_storage_and_skill_discovery() {
        let home = std::env::temp_dir().join(format!("kimi-bootstrap-{}", uuid::Uuid::new_v4()));
        let result = bootstrap_with_extra(input(&home), log_seed(&home)).unwrap();

        let bootstrap = result.app.get(BOOTSTRAP_SERVICE_ID).unwrap();
        assert_eq!(bootstrap.home_dir(), home);

        let storage = result.app.get(FILE_SYSTEM_STORAGE_SERVICE_ID).unwrap();
        storage
            .0
            .write(
                "test",
                "value.txt",
                b"persisted",
                StorageWriteOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read(home.join("test/value.txt")).await.unwrap(),
            b"persisted"
        );

        result.app.get(SKILL_DISCOVERY_SERVICE_ID).unwrap();
        result.app.dispose().unwrap();
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn extra_seed_overrides_bootstrap_defaults() {
        let default_home = Path::new("/tmp/default-home");
        let override_home = Path::new("/tmp/override-home");
        let mut extra = ServiceCollection::new();
        extra.set_instance(
            BOOTSTRAP_OPTIONS_ID,
            Arc::new(
                resolve_bootstrap_options(input(override_home))
                    .expect("override options should resolve"),
            ),
        );

        let result = bootstrap_with_extra(input(default_home), extra).unwrap();
        assert_eq!(
            result.app.get(BOOTSTRAP_SERVICE_ID).unwrap().home_dir(),
            override_home
        );
        result.app.dispose().unwrap();
    }
}
