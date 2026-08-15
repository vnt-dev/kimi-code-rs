//! Node-style host platform/architecture naming.
//!
//! Mirrors `process.platform` / `process.arch` (Node's `os.platform()` /
//! `os.arch()`) for code ported from TypeScript that needs the host platform
//! as **data** (startup snapshot, asset-name lookup, error messages).
//! Use `cfg!(...)` / `#[cfg(...)]` instead when the code merely branches on
//! the host OS — which is the norm.

/// `process.platform`-style name for the host OS.
pub fn node_platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        "illumos" => "sunos",
        other => other,
    }
}

/// `process.arch`-style name for the host architecture.
pub fn node_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" => "ia32",
        "aarch64" => "arm64",
        "loongarch64" => "loong64",
        "powerpc" => "ppc",
        "powerpc64" => "ppc64",
        other => other,
    }
}

// Each CI runner verifies the mapping for its own target.
#[cfg(target_os = "windows")]
#[test]
fn maps_windows_to_win32() {
    assert_eq!(node_platform(), "win32");
}

#[cfg(target_os = "macos")]
#[test]
fn maps_macos_to_darwin() {
    assert_eq!(node_platform(), "darwin");
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
#[test]
fn maps_linux_arm_to_arm64() {
    assert_eq!(node_platform(), "linux");
    assert_eq!(node_arch(), "arm64");
}
