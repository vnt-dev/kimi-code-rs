//! Legacy server-sent-events framing for MCP transports.
//!
//! Original: `agent/mcp/client-sse.ts`, `SSEClientTransport` input stream.

/// A complete server-sent event after line folding and blank-line dispatch.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct McpSseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum McpSseEndpointError {
    #[error("invalid MCP SSE server URL: {0}")]
    ServerUrl(#[from] url::ParseError),
    #[error("MCP SSE endpoint event has an empty message URL")]
    EmptyEndpoint,
}

/// Resolves the legacy `event: endpoint` data field using the source event
/// stream as the base URL, as done by the MCP SDK's SSE client transport.
pub fn resolve_sse_message_endpoint(
    server_url: &str,
    endpoint: &str,
) -> Result<url::Url, McpSseEndpointError> {
    if endpoint.is_empty() {
        return Err(McpSseEndpointError::EmptyEndpoint);
    }
    let server_url = url::Url::parse(server_url)?;
    Ok(server_url.join(endpoint)?)
}

/// Incremental SSE decoder used by the legacy MCP client. It accepts arbitrary
/// UTF-8 chunk boundaries and follows the SSE field/blank-line dispatch rules.
#[derive(Default)]
pub struct McpSseDecoder {
    pending: String,
    event: Option<String>,
    data: Vec<String>,
    id: Option<String>,
}

impl McpSseDecoder {
    // Original adaptation: SSEClientTransport's event-source parser.
    pub fn push(&mut self, chunk: &str) -> Vec<McpSseEvent> {
        self.pending.push_str(chunk);
        let mut events = Vec::new();
        while let Some(newline) = self.pending.find('\n') {
            let mut line = self.pending[..newline].to_owned();
            self.pending.drain(..=newline);
            if line.ends_with('\r') {
                line.pop();
            }
            if line.is_empty() {
                if let Some(event) = self.dispatch() {
                    events.push(event);
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = line.split_once(':').unwrap_or((line.as_str(), ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => self.event = Some(value.into()),
                "data" => self.data.push(value.into()),
                "id" if !value.contains('\0') => self.id = Some(value.into()),
                _ => {}
            }
        }
        events
    }

    pub fn finish(&mut self) -> Option<McpSseEvent> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.apply_line(&line);
        }
        self.dispatch()
    }

    fn apply_line(&mut self, line: &str) {
        if line.is_empty() || line.starts_with(':') {
            return;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = Some(value.into()),
            "data" => self.data.push(value.into()),
            "id" if !value.contains('\0') => self.id = Some(value.into()),
            _ => {}
        }
    }

    fn dispatch(&mut self) -> Option<McpSseEvent> {
        if self.data.is_empty() {
            self.event = None;
            return None;
        }
        Some(McpSseEvent {
            event: self.event.take(),
            data: std::mem::take(&mut self.data).join("\n"),
            id: self.id.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_folded_events_across_arbitrary_chunks() {
        let mut decoder = McpSseDecoder::default();
        assert!(decoder.push("event: endpoint\ndata: /m").is_empty());
        assert_eq!(
            decoder.push("essages\nid: 7\n\ndata: {\\\"jsonrpc\\\":\\\"2.0\\\"}\n\n"),
            vec![
                McpSseEvent {
                    event: Some("endpoint".into()),
                    data: "/messages".into(),
                    id: Some("7".into()),
                },
                McpSseEvent {
                    event: None,
                    data: r#"{\"jsonrpc\":\"2.0\"}"#.into(),
                    id: Some("7".into()),
                },
            ]
        );
    }

    #[test]
    fn ignores_comments_and_null_ids_and_finishes_unterminated_data() {
        let mut decoder = McpSseDecoder::default();
        assert!(
            decoder
                .push(": ping\nid: good\nid: bad\0id\ndata: final")
                .is_empty()
        );
        assert_eq!(
            decoder.finish(),
            Some(McpSseEvent {
                event: None,
                data: "final".into(),
                id: Some("good".into()),
            })
        );
    }

    #[test]
    fn resolves_endpoint_events_against_the_sse_url() {
        assert_eq!(
            resolve_sse_message_endpoint("https://mcp.example/events", "/messages").unwrap(),
            url::Url::parse("https://mcp.example/messages").unwrap()
        );
        assert_eq!(
            resolve_sse_message_endpoint("https://mcp.example/base/events", "messages").unwrap(),
            url::Url::parse("https://mcp.example/base/messages").unwrap()
        );
        assert!(matches!(
            resolve_sse_message_endpoint("https://mcp.example/events", ""),
            Err(McpSseEndpointError::EmptyEndpoint)
        ));
    }
}
