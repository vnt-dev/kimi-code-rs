use std::collections::HashSet;

use crate::sdk::types::{ToolUpdate, ToolUpdateKind};

pub const MCP_OAUTH_AUTHORIZATION_URL_TOOL_UPDATE: &str = "mcp.oauth.authorization_url";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOauthAuthorizationUrlUpdateData {
    pub server_name: String,
    pub authorization_url: String,
}

/// Original:
///   apps/kimi-code/src/tui/utils/mcp-oauth.ts
///   parseMcpOAuthAuthorizationUrlUpdate()
pub fn parse_mcp_oauth_authorization_url_update(
    update: &ToolUpdate,
) -> Option<McpOauthAuthorizationUrlUpdateData> {
    if update.kind != ToolUpdateKind::Custom
        || update.custom_kind.as_deref() != Some(MCP_OAUTH_AUTHORIZATION_URL_TOOL_UPDATE)
    {
        return None;
    }
    let data = update.custom_data.as_ref()?.as_object()?;
    let server_name = data.get("serverName")?.as_str()?;
    let authorization_url = data.get("authorizationUrl")?.as_str()?;
    if server_name.is_empty() || authorization_url.is_empty() || !is_http_url(authorization_url) {
        return None;
    }
    Some(McpOauthAuthorizationUrlUpdateData {
        server_name: server_name.to_owned(),
        authorization_url: authorization_url.to_owned(),
    })
}

fn is_http_url(value: &str) -> bool {
    url::Url::parse(value)
        .ok()
        .is_some_and(|url| matches!(url.scheme(), "http" | "https"))
}

pub struct McpOauthAuthorizationUrlOpener<F> {
    open_url: F,
    opened_authorization_urls: HashSet<String>,
}

impl<F> McpOauthAuthorizationUrlOpener<F>
where
    F: FnMut(&str),
{
    pub fn new(open_url: F) -> Self {
        Self {
            open_url,
            opened_authorization_urls: HashSet::new(),
        }
    }

    /// Original:
    ///   apps/kimi-code/src/tui/utils/mcp-oauth.ts
    ///   McpOAuthAuthorizationUrlOpener.handleToolProgress()
    pub fn handle_tool_progress(&mut self, tool_call_id: &str, update: &ToolUpdate) {
        let Some(update) = parse_mcp_oauth_authorization_url_update(update) else {
            return;
        };
        let key = format!("{tool_call_id}\0{}", update.authorization_url);
        if !self.opened_authorization_urls.insert(key) {
            return;
        }
        (self.open_url)(&update.authorization_url);
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use serde_json::json;

    use super::*;

    fn authorization_update(url: &str) -> ToolUpdate {
        ToolUpdate {
            kind: ToolUpdateKind::Custom,
            text: None,
            percent: None,
            custom_kind: Some(MCP_OAUTH_AUTHORIZATION_URL_TOOL_UPDATE.to_owned()),
            custom_data: Some(json!({
                "serverName": "linear",
                "authorizationUrl": url,
            })),
        }
    }

    #[test]
    fn parses_only_structured_http_authorization_updates() {
        assert_eq!(
            parse_mcp_oauth_authorization_url_update(&authorization_update(
                "https://linear.example/oauth?state=abc"
            )),
            Some(McpOauthAuthorizationUrlUpdateData {
                server_name: "linear".to_owned(),
                authorization_url: "https://linear.example/oauth?state=abc".to_owned(),
            })
        );
        assert_eq!(
            parse_mcp_oauth_authorization_url_update(&authorization_update("file:///tmp/callback")),
            None
        );
        let unrelated = ToolUpdate {
            kind: ToolUpdateKind::Status,
            text: Some("https://linear.example/oauth".to_owned()),
            percent: None,
            custom_kind: None,
            custom_data: None,
        };
        assert_eq!(parse_mcp_oauth_authorization_url_update(&unrelated), None);
    }

    #[test]
    fn opens_each_url_once_per_tool_call() {
        let opened = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&opened);
        let mut opener = McpOauthAuthorizationUrlOpener::new(move |url: &str| {
            recorded.borrow_mut().push(url.to_owned());
        });
        let update = authorization_update("https://linear.example/oauth?state=abc");

        opener.handle_tool_progress("tool-1", &update);
        opener.handle_tool_progress("tool-1", &update);
        opener.handle_tool_progress("tool-2", &update);

        assert_eq!(opened.borrow().len(), 2);
        assert!(
            opened
                .borrow()
                .iter()
                .all(|url| url == "https://linear.example/oauth?state=abc")
        );
    }

    #[test]
    fn ignores_updates_without_a_valid_authorization_url() {
        let mut opened = Vec::new();
        {
            let mut opener = McpOauthAuthorizationUrlOpener::new(|url: &str| {
                opened.push(url.to_owned());
            });
            opener.handle_tool_progress("tool-1", &authorization_update("file:///tmp/a"));
        }
        assert!(opened.is_empty());
    }
}
