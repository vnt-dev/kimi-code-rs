use std::collections::HashMap;

use super::types::OAuthFlowConfig;

pub const DEFAULT_KIMI_CODE_OAUTH_HOST: &str = "https://auth.kimi.com";
pub const KIMI_CODE_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";

// Original:
//   packages/oauth/src/constants.ts
//   KIMI_CODE_FLOW_CONFIG
//
// Rust adaptation:
//   The TypeScript value is initialized when its module loads. Rust resolves
//   the process environment when application composition asks for the config.
pub fn kimi_code_flow_config() -> OAuthFlowConfig {
    kimi_code_flow_config_from(&std::env::vars().collect())
}

pub fn kimi_code_flow_config_from(environment: &HashMap<String, String>) -> OAuthFlowConfig {
    let oauth_host = environment
        .get("KIMI_CODE_OAUTH_HOST")
        .or_else(|| environment.get("KIMI_OAUTH_HOST"))
        .cloned()
        .unwrap_or_else(|| DEFAULT_KIMI_CODE_OAUTH_HOST.to_owned());
    OAuthFlowConfig {
        name: "kimi-code".to_owned(),
        oauth_host,
        client_id: KIMI_CODE_CLIENT_ID.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_the_production_host_and_registered_client() {
        let config = kimi_code_flow_config_from(&HashMap::new());
        assert_eq!(config.name, "kimi-code");
        assert_eq!(config.oauth_host, DEFAULT_KIMI_CODE_OAUTH_HOST);
        assert_eq!(config.client_id, KIMI_CODE_CLIENT_ID);
    }

    #[test]
    fn primary_host_environment_wins_over_the_legacy_alias_without_trimming() {
        let environment = HashMap::from([
            ("KIMI_CODE_OAUTH_HOST".to_owned(), "".to_owned()),
            (
                "KIMI_OAUTH_HOST".to_owned(),
                "https://legacy.example".to_owned(),
            ),
        ]);
        assert_eq!(
            kimi_code_flow_config_from(&environment).oauth_host,
            "",
            "JavaScript nullish coalescing preserves an explicitly empty value"
        );

        let legacy_only = HashMap::from([(
            "KIMI_OAUTH_HOST".to_owned(),
            " https://legacy.example/ ".to_owned(),
        )]);
        assert_eq!(
            kimi_code_flow_config_from(&legacy_only).oauth_host,
            " https://legacy.example/ "
        );
    }
}
