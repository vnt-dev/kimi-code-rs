use std::collections::HashMap;

use thiserror::Error;

const BCRYPT_COST: u32 = 12;

#[derive(Debug, Error)]
pub enum PasswordError {
    #[error(transparent)]
    Bcrypt(#[from] bcrypt::BcryptError),
    #[error("password worker failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

// Original: packages/kap-server/src/services/auth/password.ts
pub async fn resolve_password_hash(
    env: &HashMap<String, String>,
) -> Result<Option<String>, PasswordError> {
    let Some(plaintext) = env
        .get("KIMI_CODE_PASSWORD")
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let plaintext = plaintext.clone();
    Ok(Some(
        tokio::task::spawn_blocking(move || bcrypt::hash(plaintext, BCRYPT_COST)).await??,
    ))
}

pub async fn verify_password(
    candidate: &str,
    password_hash: Option<&str>,
) -> Result<bool, PasswordError> {
    let Some(password_hash) = password_hash else {
        return Ok(false);
    };
    let candidate = candidate.to_owned();
    let password_hash = password_hash.to_owned();
    Ok(tokio::task::spawn_blocking(move || bcrypt::verify(candidate, &password_hash)).await??)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hashes_and_verifies_configured_password() {
        assert_eq!(resolve_password_hash(&HashMap::new()).await.unwrap(), None);
        let env = HashMap::from([(
            "KIMI_CODE_PASSWORD".to_owned(),
            "correct-horse-battery-staple".to_owned(),
        )]);
        let hash = resolve_password_hash(&env).await.unwrap().unwrap();
        assert!(hash.starts_with("$2"));
        assert!(
            verify_password("correct-horse-battery-staple", Some(&hash))
                .await
                .unwrap()
        );
        assert!(!verify_password("wrong", Some(&hash)).await.unwrap());
        assert!(!verify_password("anything", None).await.unwrap());
    }
}
