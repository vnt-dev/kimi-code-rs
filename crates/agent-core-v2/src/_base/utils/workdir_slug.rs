use super::hash::sha256_hex_prefix;

const MAX_WORKDIR_SLUG_LENGTH: usize = 40;
const WORKDIR_KEY_PREFIX: &str = "wd_";
const HASH_LENGTH: usize = 12;

// Original:
//   packages/agent-core-v2/src/_base/utils/workdir-slug.ts
//   slugifyWorkDirName()
pub fn slugify_work_dir_name(name: &str) -> String {
    let mut slug = String::new();
    let mut inside_disallowed_run = false;
    for character in name.to_lowercase().chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            slug.push(character);
            inside_disallowed_run = false;
        } else if !inside_disallowed_run {
            slug.push('-');
            inside_disallowed_run = true;
        }
    }

    let slug = slug.trim_matches('-');
    let truncated = &slug[..slug.len().min(MAX_WORKDIR_SLUG_LENGTH)];
    let slug = truncated.trim_matches('-');
    if slug.is_empty() || matches!(slug, "." | "..") {
        "workspace".to_owned()
    } else {
        slug.to_owned()
    }
}

// Original:
//   packages/agent-core-v2/src/_base/utils/workdir-slug.ts
//   encodeWorkDirKey()
pub fn encode_work_dir_key(work_dir: &str) -> String {
    let slashed = work_dir.replace('\\', "/");
    let normalized = slashed.trim_end_matches('/');
    let base = normalized.rsplit('/').next().unwrap_or(normalized);
    let slug = slugify_work_dir_name(base);
    let hash = sha256_hex_prefix(normalized.as_bytes(), HASH_LENGTH / 2);
    format!("{WORKDIR_KEY_PREFIX}{slug}_{hash}")
}

// Original:
//   packages/agent-core-v2/src/_base/utils/workdir-slug.ts
//   workspaceRootKey()
pub fn workspace_root_key(root: &str) -> String {
    let slashed = root.replace('\\', "/");
    let windows_shaped = is_windows_shaped(&slashed);
    let normalized = slashed.trim_end_matches('/');
    if windows_shaped {
        normalized.to_lowercase()
    } else {
        normalized.to_owned()
    }
}

fn is_windows_shaped(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/')
        || path.starts_with("//")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugifies_with_source_character_rules_and_length_limit() {
        assert_eq!(slugify_work_dir_name("My Project"), "my-project");
        assert_eq!(slugify_work_dir_name(" a   b "), "a-b");
        assert_eq!(slugify_work_dir_name("a - b"), "a---b");
        assert_eq!(
            slugify_work_dir_name("ABCDEFGHIJKLMNOPQRSTUVWXYZ-abcdefghijklmnop"),
            "abcdefghijklmnopqrstuvwxyz-abcdefghijklm"
        );
    }

    #[test]
    fn reserved_or_empty_slugs_fall_back_to_workspace() {
        for name in ["", ".", "..", "---", "项目"] {
            assert_eq!(slugify_work_dir_name(name), "workspace");
        }
    }

    #[test]
    fn encodes_stable_source_compatible_workspace_ids() {
        assert_eq!(
            encode_work_dir_key("/tmp/My Project"),
            "wd_my-project_dab36f6a753b"
        );
        assert_eq!(
            encode_work_dir_key(r"C:\Users\Foo\Proj\"),
            encode_work_dir_key("C:/Users/Foo/Proj/")
        );
        assert_ne!(
            encode_work_dir_key("C:/Users/Foo/Proj"),
            encode_work_dir_key("c:/users/foo/proj")
        );
    }

    #[test]
    fn workspace_root_key_folds_windows_drive_paths() {
        assert_eq!(
            workspace_root_key(r"C:\Users\Foo\Proj"),
            "c:/users/foo/proj"
        );
        assert_eq!(
            workspace_root_key("c:/Users/Foo/Proj/"),
            "c:/users/foo/proj"
        );
        assert_eq!(workspace_root_key(r"C:\"), "c:");
    }

    #[test]
    fn workspace_root_key_folds_unc_but_preserves_posix_case() {
        assert_eq!(workspace_root_key(r"\\HOST\Share\Dir"), "//host/share/dir");
        assert_eq!(workspace_root_key("//HOST/Share/Dir/"), "//host/share/dir");
        assert_eq!(workspace_root_key("/tmp/Foo/"), "/tmp/Foo");
        assert_ne!(
            workspace_root_key("/tmp/Foo"),
            workspace_root_key("/tmp/foo")
        );
    }
}
