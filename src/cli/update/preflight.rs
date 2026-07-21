use std::{error::Error, fmt};

use super::{
    prompt::CHANGELOG_URL,
    types::{InstallSource, NPM_PACKAGE_NAME, UpdateTarget},
};

pub const KIMI_CODE_CDN_BASE: &str = "https://code.kimi.com/kimi-code";
pub const NATIVE_INSTALL_COMMAND_UNIX: &str =
    "curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash";
pub const NATIVE_INSTALL_COMMAND_WIN: &str =
    "irm https://code.kimi.com/kimi-code/install.ps1 | iex";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdatePlatform {
    Windows,
    Other,
}

impl UpdatePlatform {
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnCommand {
    pub command: String,
    pub arguments: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedInstallSource;

impl fmt::Display for UnsupportedInstallSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("unsupported install source cannot be auto-installed")
    }
}

impl Error for UnsupportedInstallSource {}

// Original:
//   apps/kimi-code/src/cli/update/preflight.ts
//   installCommandFor()
pub fn install_command_for(
    source: InstallSource,
    version: &str,
    platform: UpdatePlatform,
) -> String {
    match source {
        InstallSource::NpmGlobal => {
            format!("npm install -g {NPM_PACKAGE_NAME}@{version}")
        }
        InstallSource::PnpmGlobal => {
            format!("pnpm add -g {NPM_PACKAGE_NAME}@{version}")
        }
        InstallSource::YarnGlobal => {
            format!("yarn global add {NPM_PACKAGE_NAME}@{version}")
        }
        InstallSource::BunGlobal => format!("bun add -g {NPM_PACKAGE_NAME}@{version}"),
        InstallSource::Homebrew => "brew upgrade kimi-code".to_owned(),
        InstallSource::Native if platform == UpdatePlatform::Windows => {
            NATIVE_INSTALL_COMMAND_WIN.to_owned()
        }
        InstallSource::Native => NATIVE_INSTALL_COMMAND_UNIX.to_owned(),
        InstallSource::Unsupported => {
            format!("npm install -g {NPM_PACKAGE_NAME}@{version}")
        }
    }
}

// Original: canAutoInstall()
pub fn can_auto_install(source: InstallSource, platform: UpdatePlatform) -> bool {
    match source {
        InstallSource::NpmGlobal
        | InstallSource::PnpmGlobal
        | InstallSource::YarnGlobal
        | InstallSource::BunGlobal => true,
        InstallSource::Homebrew | InstallSource::Unsupported => false,
        InstallSource::Native => platform != UpdatePlatform::Windows,
    }
}

// Original: spawnForSource()
pub fn spawn_for_source(
    source: InstallSource,
    version: &str,
    platform: UpdatePlatform,
) -> Result<SpawnCommand, UnsupportedInstallSource> {
    let package = format!("{NPM_PACKAGE_NAME}@{version}");
    let command = match source {
        InstallSource::NpmGlobal => SpawnCommand {
            command: with_cmd_suffix("npm", platform),
            arguments: vec!["install".to_owned(), "-g".to_owned(), package],
        },
        InstallSource::PnpmGlobal => SpawnCommand {
            command: with_cmd_suffix("pnpm", platform),
            arguments: vec!["add".to_owned(), "-g".to_owned(), package],
        },
        InstallSource::YarnGlobal => SpawnCommand {
            command: with_cmd_suffix("yarn", platform),
            arguments: vec!["global".to_owned(), "add".to_owned(), package],
        },
        InstallSource::BunGlobal => SpawnCommand {
            command: if platform == UpdatePlatform::Windows {
                "bun.exe".to_owned()
            } else {
                "bun".to_owned()
            },
            arguments: vec!["add".to_owned(), "-g".to_owned(), package],
        },
        InstallSource::Homebrew => SpawnCommand {
            command: "brew".to_owned(),
            arguments: vec!["upgrade".to_owned(), "kimi-code".to_owned()],
        },
        InstallSource::Native => SpawnCommand {
            command: "bash".to_owned(),
            arguments: vec![
                "-c".to_owned(),
                format!("set -o pipefail; {NATIVE_INSTALL_COMMAND_UNIX}"),
            ],
        },
        InstallSource::Unsupported => return Err(UnsupportedInstallSource),
    };
    Ok(command)
}

fn with_cmd_suffix(base: &str, platform: UpdatePlatform) -> String {
    if platform == UpdatePlatform::Windows {
        format!("{base}.cmd")
    } else {
        base.to_owned()
    }
}

// Original: renderManualUpdateMessage()
pub fn render_manual_update_message(
    current_version: &str,
    target: &UpdateTarget,
    source: InstallSource,
    install_command: &str,
) -> String {
    let source_description = match source {
        InstallSource::NpmGlobal
        | InstallSource::PnpmGlobal
        | InstallSource::YarnGlobal
        | InstallSource::BunGlobal => source.as_str(),
        InstallSource::Homebrew => "homebrew",
        InstallSource::Native => "native (windows). Auto-update is not supported on this platform.",
        InstallSource::Unsupported => "unsupported package manager or layout.",
    };
    format!(
        "A newer version of {NPM_PACKAGE_NAME} is available ({current_version} -> {}).\n\
Detected install source: {source_description}\n\
To update manually, run: {install_command}\n",
        target.version
    )
}

// Original: renderInstallSuccessMessage()
pub fn render_install_success_message(target: &UpdateTarget) -> String {
    format!(
        "Updated {NPM_PACKAGE_NAME} to {}. Restart the CLI to use the new version.\n",
        target.version
    )
}

pub fn render_background_install_success_notice(version: &str) -> String {
    let display_version = if version.starts_with('v') {
        version.to_owned()
    } else {
        format!("v{version}")
    };
    format!("Kimi Code updated to {display_version}\nChangelog: {CHANGELOG_URL}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSION: &str = "0.5.0";

    #[test]
    fn builds_manual_commands_for_every_install_source() {
        assert_eq!(
            install_command_for(InstallSource::NpmGlobal, VERSION, UpdatePlatform::Other),
            "npm install -g @moonshot-ai/kimi-code@0.5.0"
        );
        assert_eq!(
            install_command_for(InstallSource::PnpmGlobal, VERSION, UpdatePlatform::Other),
            "pnpm add -g @moonshot-ai/kimi-code@0.5.0"
        );
        assert_eq!(
            install_command_for(InstallSource::YarnGlobal, VERSION, UpdatePlatform::Other),
            "yarn global add @moonshot-ai/kimi-code@0.5.0"
        );
        assert_eq!(
            install_command_for(InstallSource::BunGlobal, VERSION, UpdatePlatform::Other),
            "bun add -g @moonshot-ai/kimi-code@0.5.0"
        );
        assert_eq!(
            install_command_for(InstallSource::Homebrew, VERSION, UpdatePlatform::Other),
            "brew upgrade kimi-code"
        );
        assert_eq!(
            install_command_for(InstallSource::Native, VERSION, UpdatePlatform::Windows),
            NATIVE_INSTALL_COMMAND_WIN
        );
        assert_eq!(
            install_command_for(InstallSource::Native, VERSION, UpdatePlatform::Other),
            NATIVE_INSTALL_COMMAND_UNIX
        );
        assert_eq!(
            install_command_for(InstallSource::Unsupported, VERSION, UpdatePlatform::Other),
            "npm install -g @moonshot-ai/kimi-code@0.5.0"
        );
    }

    #[test]
    fn auto_install_capability_matches_package_manager_and_platform() {
        for source in [
            InstallSource::NpmGlobal,
            InstallSource::PnpmGlobal,
            InstallSource::YarnGlobal,
            InstallSource::BunGlobal,
        ] {
            assert!(can_auto_install(source, UpdatePlatform::Windows));
            assert!(can_auto_install(source, UpdatePlatform::Other));
        }
        assert!(!can_auto_install(
            InstallSource::Homebrew,
            UpdatePlatform::Other
        ));
        assert!(!can_auto_install(
            InstallSource::Unsupported,
            UpdatePlatform::Other
        ));
        assert!(!can_auto_install(
            InstallSource::Native,
            UpdatePlatform::Windows
        ));
        assert!(can_auto_install(
            InstallSource::Native,
            UpdatePlatform::Other
        ));
    }

    #[test]
    fn builds_spawn_commands_and_windows_executable_suffixes() {
        assert_eq!(
            spawn_for_source(InstallSource::NpmGlobal, VERSION, UpdatePlatform::Windows)
                .expect("npm"),
            SpawnCommand {
                command: "npm.cmd".to_owned(),
                arguments: vec![
                    "install".to_owned(),
                    "-g".to_owned(),
                    "@moonshot-ai/kimi-code@0.5.0".to_owned()
                ],
            }
        );
        assert_eq!(
            spawn_for_source(InstallSource::BunGlobal, VERSION, UpdatePlatform::Windows)
                .expect("bun"),
            SpawnCommand {
                command: "bun.exe".to_owned(),
                arguments: vec![
                    "add".to_owned(),
                    "-g".to_owned(),
                    "@moonshot-ai/kimi-code@0.5.0".to_owned()
                ],
            }
        );
        assert_eq!(
            spawn_for_source(InstallSource::Homebrew, VERSION, UpdatePlatform::Other)
                .expect("brew"),
            SpawnCommand {
                command: "brew".to_owned(),
                arguments: vec!["upgrade".to_owned(), "kimi-code".to_owned()],
            }
        );
        assert_eq!(
            spawn_for_source(InstallSource::Native, VERSION, UpdatePlatform::Other)
                .expect("native")
                .arguments,
            [
                "-c".to_owned(),
                format!("set -o pipefail; {NATIVE_INSTALL_COMMAND_UNIX}")
            ]
        );
        assert!(
            spawn_for_source(InstallSource::Unsupported, VERSION, UpdatePlatform::Other).is_err()
        );
    }

    #[test]
    fn renders_manual_success_and_background_messages() {
        let target = UpdateTarget {
            version: VERSION.to_owned(),
        };
        assert_eq!(
            render_manual_update_message(
                "0.4.0",
                &target,
                InstallSource::Native,
                NATIVE_INSTALL_COMMAND_WIN
            ),
            format!(
                "A newer version of @moonshot-ai/kimi-code is available (0.4.0 -> 0.5.0).\nDetected install source: native (windows). Auto-update is not supported on this platform.\nTo update manually, run: {NATIVE_INSTALL_COMMAND_WIN}\n"
            )
        );
        assert_eq!(
            render_install_success_message(&target),
            "Updated @moonshot-ai/kimi-code to 0.5.0. Restart the CLI to use the new version.\n"
        );
        assert_eq!(
            render_background_install_success_notice("0.5.0"),
            format!("Kimi Code updated to v0.5.0\nChangelog: {CHANGELOG_URL}\n")
        );
        assert!(render_background_install_success_notice("v0.5.0").contains("to v0.5.0"));
    }
}
