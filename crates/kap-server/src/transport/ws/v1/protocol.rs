use kimi_code_protocol::{
    ResyncRequiredMessage, ResyncRequiredPayload, ResyncRequiredReason, ResyncRequiredType,
    ServerHelloCapabilities, ServerHelloMessage, ServerHelloPayload, ServerHelloType,
    WsAckEnvelope, WsAckType, now_iso_date_time,
};

// Original: transport/ws/v1/protocol.ts, buildServerHello().
pub fn build_server_hello(payload: ServerHelloPayload) -> ServerHelloMessage {
    ServerHelloMessage {
        message_type: ServerHelloType::ServerHello,
        timestamp: now_iso_date_time(),
        payload,
    }
}

pub fn build_ack<T>(
    id: impl Into<String>,
    code: i64,
    message: impl Into<String>,
    payload: T,
) -> WsAckEnvelope<T> {
    WsAckEnvelope {
        ack_type: WsAckType::Ack,
        id: id.into(),
        code,
        msg: message.into(),
        payload,
    }
}

pub fn build_resync_required(
    session_id: impl Into<String>,
    reason: ResyncRequiredReason,
    current_seq: u64,
    epoch: Option<String>,
) -> ResyncRequiredMessage {
    ResyncRequiredMessage {
        message_type: ResyncRequiredType::ResyncRequired,
        timestamp: now_iso_date_time(),
        payload: ResyncRequiredPayload {
            session_id: session_id.into(),
            reason,
            current_seq,
            epoch,
        },
    }
}

pub fn default_server_hello_payload(
    connection_id: impl Into<String>,
    protocol_version: u64,
    max_event_buffer_size: u64,
) -> ServerHelloPayload {
    ServerHelloPayload {
        ws_connection_id: connection_id.into(),
        protocol_version,
        heartbeat_ms: None,
        max_event_buffer_size,
        capabilities: ServerHelloCapabilities {
            event_batching: true,
            compression: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_wire_owned_frames() {
        let hello = build_server_hello(default_server_hello_payload("conn_1", 2, 1_000));
        let json = serde_json::to_value(hello).unwrap();
        assert_eq!(json["type"], "server_hello");
        assert_eq!(json["payload"]["capabilities"]["event_batching"], true);

        let ack = build_ack("a1", 0, "success", serde_json::json!({"ok": true}));
        assert_eq!(serde_json::to_value(ack).unwrap()["type"], "ack");

        let resync = build_resync_required(
            "s1",
            ResyncRequiredReason::EpochChanged,
            42,
            Some("ep_1".into()),
        );
        let json = serde_json::to_value(resync).unwrap();
        assert_eq!(json["type"], "resync_required");
        assert_eq!(json["payload"]["reason"], "epoch_changed");
    }
}
