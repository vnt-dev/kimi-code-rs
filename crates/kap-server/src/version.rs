/// Rust counterpart of `version.ts`.
///
/// Cargo embeds the package version at compile time, so no blocking package
/// manifest read or process-global cache is required.
pub const fn get_server_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_cargo_package_version() {
        assert_eq!(get_server_version(), "0.1.0");
    }
}
