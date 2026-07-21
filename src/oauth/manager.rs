const MIN_REFRESH_THRESHOLD_SECONDS: f64 = 300.0;
const REFRESH_THRESHOLD_RATIO: f64 = 0.5;

// Original:
//   packages/oauth/src/oauth-manager.ts
//   defaultRefreshThreshold()
pub fn default_refresh_threshold(expires_in: f64) -> f64 {
    if expires_in > 0.0 {
        MIN_REFRESH_THRESHOLD_SECONDS.max(expires_in * REFRESH_THRESHOLD_RATIO)
    } else {
        MIN_REFRESH_THRESHOLD_SECONDS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_threshold_uses_half_the_lifetime_with_a_five_minute_floor() {
        for (expires_in, expected) in [
            (-1.0, 300.0),
            (0.0, 300.0),
            (1.0, 300.0),
            (600.0, 300.0),
            (3_600.0, 1_800.0),
        ] {
            assert_eq!(default_refresh_threshold(expires_in), expected);
        }
    }

    #[test]
    fn refresh_threshold_preserves_javascript_nan_and_infinity_edges() {
        assert_eq!(default_refresh_threshold(f64::NAN), 300.0);
        assert_eq!(default_refresh_threshold(f64::NEG_INFINITY), 300.0);
        assert_eq!(default_refresh_threshold(f64::INFINITY), f64::INFINITY);
    }
}
