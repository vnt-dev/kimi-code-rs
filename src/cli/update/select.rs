use semver::Version;

use super::types::UpdateTarget;

// Original:
//   apps/kimi-code/src/cli/update/select.ts
//   selectUpdateTarget()
pub fn select_update_target(current_version: &str, latest: Option<&str>) -> Option<UpdateTarget> {
    let latest_text = latest?;
    let current = Version::parse(current_version).ok()?;
    let latest = Version::parse(latest_text).ok()?;
    if latest <= current {
        return None;
    }
    Some(UpdateTarget {
        version: latest_text.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_only_a_strictly_newer_valid_version() {
        assert_eq!(
            select_update_target("0.4.0", Some("0.5.0")),
            Some(UpdateTarget {
                version: "0.5.0".to_owned()
            })
        );
        assert_eq!(select_update_target("0.5.0", Some("0.5.0")), None);
        assert_eq!(select_update_target("0.6.0", Some("0.5.0")), None);
        assert_eq!(select_update_target("0.5.0", None), None);
    }

    #[test]
    fn rejects_invalid_current_and_latest_versions() {
        assert_eq!(select_update_target("not-a-version", Some("0.5.0")), None);
        assert_eq!(select_update_target("0.5.0", Some("not-a-version")), None);
    }

    #[test]
    fn compares_prereleases_using_semver_precedence() {
        assert_eq!(
            select_update_target("0.5.0-rc.1", Some("0.5.0")),
            Some(UpdateTarget {
                version: "0.5.0".to_owned()
            })
        );
        assert_eq!(select_update_target("0.5.0", Some("0.5.0-rc.1")), None);
    }
}
