//! Agent-core-v2 package version resolution.
//!
//! Original: `packages/agent-core-v2/src/app/telemetry/coreVersion.ts`.

// Original: resolveCoreVersion(). Cargo embeds the owning package version at
// compile time, so the Rust artifact remains attributable after installation
// or bundling without runtime filesystem walking or package.json access.
pub const fn resolve_core_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_migrated_source_package_version() {
        assert_eq!(resolve_core_version(), "0.1.2");
        assert_eq!(resolve_core_version(), resolve_core_version());
    }
}
