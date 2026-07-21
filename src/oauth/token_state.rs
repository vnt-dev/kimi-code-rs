use super::types::TokenInfo;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenState {
    Valid(TokenInfo),
    Revoked { scope: String, token_type: String },
    Missing,
}

// Original:
//   packages/oauth/src/token-state.ts
//   classifyToken()
pub fn classify_token(token: Option<TokenInfo>) -> TokenState {
    match token {
        None => TokenState::Missing,
        Some(token) if token.access_token.is_empty() => TokenState::Revoked {
            scope: token.scope,
            token_type: token.token_type,
        },
        Some(token) => TokenState::Valid(token),
    }
}

// Original: revokedTombstone()
pub fn revoked_tombstone(prior: &TokenInfo) -> TokenInfo {
    TokenInfo {
        access_token: String::new(),
        refresh_token: String::new(),
        expires_at: 0.0,
        scope: prior.scope.clone(),
        token_type: prior.token_type.clone(),
        expires_in: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(access_token: &str) -> TokenInfo {
        TokenInfo {
            access_token: access_token.to_owned(),
            refresh_token: "refresh".to_owned(),
            expires_at: 10.0,
            scope: "scope".to_owned(),
            token_type: "Bearer".to_owned(),
            expires_in: 10.0,
        }
    }

    #[test]
    fn classifies_missing_valid_and_revoked_tokens() {
        assert_eq!(classify_token(None), TokenState::Missing);
        assert!(matches!(
            classify_token(Some(token("access"))),
            TokenState::Valid(_)
        ));
        assert_eq!(
            classify_token(Some(token(""))),
            TokenState::Revoked {
                scope: "scope".to_owned(),
                token_type: "Bearer".to_owned()
            }
        );
    }

    #[test]
    fn revoked_tombstone_clears_credentials_and_preserves_metadata() {
        assert_eq!(
            revoked_tombstone(&token("access")),
            TokenInfo {
                access_token: String::new(),
                refresh_token: String::new(),
                expires_at: 0.0,
                scope: "scope".to_owned(),
                token_type: "Bearer".to_owned(),
                expires_in: 0.0,
            }
        );
    }
}
