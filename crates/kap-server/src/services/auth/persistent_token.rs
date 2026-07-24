use std::io;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use super::private_files::{PrivateFileError, read_private_file, write_private_file};

pub const SERVER_TOKEN_FILE: &str = "server.token";

pub fn server_token_path(home_dir: impl AsRef<Path>) -> PathBuf {
    home_dir.as_ref().join(SERVER_TOKEN_FILE)
}

/// Generate the same 256-bit, unpadded base64url token shape as Node.
pub fn generate_server_token() -> String {
    let mut bytes = [0_u8; 32];
    // Node's randomBytes() throws when the operating-system RNG is
    // unavailable. Keep the same intentionally unrecoverable behavior.
    getrandom::fill(&mut bytes).expect("operating-system randomness is unavailable");
    URL_SAFE_NO_PAD.encode(bytes)
}

pub async fn write_server_token(
    home_dir: impl AsRef<Path>,
    token: &str,
) -> Result<(), PrivateFileError> {
    write_private_file(server_token_path(home_dir), token).await
}

pub async fn read_server_token(
    home_dir: impl AsRef<Path>,
) -> Result<Option<String>, PrivateFileError> {
    match read_private_file(server_token_path(home_dir)).await {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).trim().to_owned())),
        Err(error) if error.io_kind() == Some(io::ErrorKind::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

// Original: persistentToken.ts, loadOrCreateServerToken().
pub async fn load_or_create_server_token(
    home_dir: impl AsRef<Path>,
) -> Result<String, PrivateFileError> {
    let home_dir = home_dir.as_ref();
    if let Some(existing) = read_server_token(home_dir).await?
        && !existing.is_empty()
    {
        return Ok(existing);
    }
    let token = generate_server_token();
    write_server_token(home_dir, &token).await?;
    Ok(token)
}

pub async fn rotate_server_token(home_dir: impl AsRef<Path>) -> Result<String, PrivateFileError> {
    let token = generate_server_token();
    write_server_token(home_dir, &token).await?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_token_is_url_safe_and_43_characters() {
        let token = generate_server_token();
        assert_eq!(token.len(), 43);
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        );
    }

    #[tokio::test]
    async fn creates_reuses_and_rotates_token() {
        let directory = tempfile::tempdir().unwrap();
        let first = load_or_create_server_token(directory.path()).await.unwrap();
        let second = load_or_create_server_token(directory.path()).await.unwrap();
        assert_eq!(first, second);

        let rotated = rotate_server_token(directory.path()).await.unwrap();
        assert_ne!(first, rotated);
        assert_eq!(
            read_server_token(directory.path()).await.unwrap(),
            Some(rotated)
        );
    }
}
