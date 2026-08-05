import { Server } from "lucide-react";
import { useEffect, useRef } from "react";

import type { McpServerInfo, McpServerStatus } from "../agentRpc";
import { t } from "../i18n";

function statusLabel(status: McpServerStatus): string {
  return t(`mcp.status.${status}`);
}

function serverDetail(server: McpServerInfo): string {
  if (server.error) return server.error;
  if (server.status === "needs-auth") {
    return t("mcp.authHint", { name: server.name });
  }
  if (server.status === "connected") {
    return t("mcp.connectedDetail", {
      transport: server.transport,
      count: server.toolCount,
    });
  }
  return server.transport;
}

export function McpStatusPopover({
  servers,
  busy,
  error,
  onClose,
}: {
  servers: readonly McpServerInfo[];
  busy: boolean;
  error?: string;
  onClose: () => void;
}) {
  const popoverRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const closeOutside = (event: PointerEvent): void => {
      if (!popoverRef.current?.contains(event.target as Node)) onClose();
    };
    const closeOnEscape = (event: KeyboardEvent): void => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("pointerdown", closeOutside);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOutside);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [onClose]);

  return (
    <div
      className="mcp-status-popover"
      ref={popoverRef}
      role="dialog"
      aria-label={t("mcp.title")}
    >
      <header>
        <span>
          <Server size={15} />
          <strong>{t("mcp.title")}</strong>
        </span>
        {!busy && !error && (
          <small>{t("mcp.serverCount", { count: servers.length })}</small>
        )}
      </header>

      <div className="mcp-status-list">
        {busy ? (
          <div className="mcp-status-placeholder">
            <span className="spinner" />
            <span>{t("mcp.loading")}</span>
          </div>
        ) : error ? (
          <div className="mcp-status-placeholder error">{error}</div>
        ) : servers.length === 0 ? (
          <div className="mcp-status-placeholder">{t("mcp.empty")}</div>
        ) : (
          servers.map((server) => (
            <div className="mcp-status-row" key={server.name}>
              <div>
                <strong>{server.name}</strong>
                <small title={serverDetail(server)}>{serverDetail(server)}</small>
              </div>
              <span className={`mcp-status-badge ${server.status}`}>
                <i aria-hidden="true" />
                {statusLabel(server.status)}
              </span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
