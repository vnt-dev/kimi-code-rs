use std::sync::{
    LazyLock,
    atomic::{AtomicBool, Ordering},
};

static RENDER_CACHE_ENABLED: LazyLock<AtomicBool> = LazyLock::new(|| {
    AtomicBool::new(render_cache_enabled_from_value(
        std::env::var("KIMI_TUI_NO_RENDER_CACHE").ok().as_deref(),
    ))
});

/// Original:
///   apps/kimi-code/src/tui/utils/render-cache.ts
///   isRenderCacheEnabled()
pub fn is_render_cache_enabled() -> bool {
    RENDER_CACHE_ENABLED.load(Ordering::Relaxed)
}

/// Intended for benchmarks and tests, matching the original runtime override.
///
/// Original:
///   apps/kimi-code/src/tui/utils/render-cache.ts
///   setRenderCacheEnabled()
pub fn set_render_cache_enabled(value: bool) {
    RENDER_CACHE_ENABLED.store(value, Ordering::Relaxed);
}

fn render_cache_enabled_from_value(disable_value: Option<&str>) -> bool {
    disable_value != Some("1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_disable_value_turns_off_default() {
        assert!(!render_cache_enabled_from_value(Some("1")));
        assert!(render_cache_enabled_from_value(Some("true")));
        assert!(render_cache_enabled_from_value(Some("0")));
        assert!(render_cache_enabled_from_value(None));
    }

    #[test]
    fn supports_runtime_override() {
        let original = is_render_cache_enabled();
        set_render_cache_enabled(false);
        assert!(!is_render_cache_enabled());
        set_render_cache_enabled(true);
        assert!(is_render_cache_enabled());
        set_render_cache_enabled(original);
    }
}
