//! Shared ripgrep binary locator.
//!
//! Original: `packages/agent-core-v2/src/session/sessionFs/rgLocator.ts`.

use std::{error::Error, path::PathBuf};

use async_trait::async_trait;

use crate::_base::utils::abort::AbortSignal;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RgResolutionSource {
    SystemPath,
    ShareBinCached,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgResolution {
    pub path: String,
    pub source: RgResolutionSource,
}

#[async_trait]
pub trait RgProbe: Send + Sync {
    async fn exec(&self, args: &[String]) -> Result<i32, Box<dyn Error + Send + Sync>>;
}

#[derive(Clone, Default)]
pub struct EnsureRgPathOptions {
    pub signal: Option<AbortSignal>,
    pub allow_cached_fallback: bool,
}

fn rg_binary_name() -> &'static str {
    if cfg!(windows) { "rg.exe" } else { "rg" }
}

fn share_dir() -> PathBuf {
    std::env::var_os("KIMI_CODE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".kimi-code")))
        .unwrap_or_else(|| PathBuf::from(".kimi-code"))
}

pub fn get_share_bin_rg_path() -> PathBuf {
    share_dir().join("bin").join(rg_binary_name())
}

fn throw_if_aborted(signal: Option<&AbortSignal>) -> Result<(), Box<dyn Error + Send + Sync>> {
    signal
        .and_then(AbortSignal::reason)
        .map_or(Ok(()), |reason| Err(Box::new((*reason).clone()) as _))
}

pub async fn ensure_rg_path(
    probe: &dyn RgProbe,
    options: EnsureRgPathOptions,
) -> Result<RgResolution, Box<dyn Error + Send + Sync>> {
    throw_if_aborted(options.signal.as_ref())?;
    let system = ["rg".into(), "--version".into()];
    if probe.exec(&system).await.unwrap_or(-1) == 0 {
        return Ok(RgResolution {
            path: "rg".into(),
            source: RgResolutionSource::SystemPath,
        });
    }

    if options.allow_cached_fallback {
        throw_if_aborted(options.signal.as_ref())?;
        let cached = get_share_bin_rg_path().to_string_lossy().into_owned();
        if probe
            .exec(&[cached.clone(), "--version".into()])
            .await
            .unwrap_or(-1)
            == 0
        {
            return Ok(RgResolution {
                path: cached,
                source: RgResolutionSource::ShareBinCached,
            });
        }
    }
    Err("ripgrep (rg) is not available on PATH".into())
}

pub fn rg_unavailable_message(cause: &(dyn Error + 'static)) -> String {
    format!(
        "ripgrep (rg) is not available.\n\nError: {cause}\n\nFix options:\n  macOS:   brew install ripgrep\n  Ubuntu:  sudo apt-get install ripgrep\n  Other:   https://github.com/BurntSushi/ripgrep#installation\n\nAlternatively, drop a static rg binary at {}",
        get_share_bin_rg_path().to_string_lossy()
    )
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use super::*;

    struct Probe {
        results: Mutex<VecDeque<i32>>,
        calls: Mutex<Vec<Vec<String>>>,
    }

    #[async_trait]
    impl RgProbe for Probe {
        async fn exec(&self, args: &[String]) -> Result<i32, Box<dyn Error + Send + Sync>> {
            self.calls.lock().unwrap().push(args.to_vec());
            Ok(self.results.lock().unwrap().pop_front().unwrap_or(-1))
        }
    }

    #[tokio::test]
    async fn system_path_wins_and_cached_fallback_is_opt_in() {
        let system = Probe {
            results: Mutex::new(VecDeque::from([0])),
            calls: Mutex::new(Vec::new()),
        };
        assert_eq!(
            ensure_rg_path(&system, EnsureRgPathOptions::default())
                .await
                .unwrap()
                .source,
            RgResolutionSource::SystemPath
        );
        assert_eq!(*system.calls.lock().unwrap(), [vec!["rg", "--version"]]);

        let cached = Probe {
            results: Mutex::new(VecDeque::from([-1, 0])),
            calls: Mutex::new(Vec::new()),
        };
        assert_eq!(
            ensure_rg_path(
                &cached,
                EnsureRgPathOptions {
                    signal: None,
                    allow_cached_fallback: true
                }
            )
            .await
            .unwrap()
            .source,
            RgResolutionSource::ShareBinCached
        );
        assert_eq!(cached.calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn abort_and_unavailable_message_preserve_source_guidance() {
        let controller = crate::_base::utils::abort::AbortController::new();
        controller.abort(None);
        let probe = Probe {
            results: Mutex::new(VecDeque::new()),
            calls: Mutex::new(Vec::new()),
        };
        assert!(
            ensure_rg_path(
                &probe,
                EnsureRgPathOptions {
                    signal: Some(controller.signal()),
                    allow_cached_fallback: true
                }
            )
            .await
            .is_err()
        );
        let message = rg_unavailable_message(&std::io::Error::other("missing"));
        assert!(message.contains("brew install ripgrep"));
        assert!(message.contains(&get_share_bin_rg_path().to_string_lossy().into_owned()));
    }
}
