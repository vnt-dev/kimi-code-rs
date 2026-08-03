import { type CSSProperties, memo, useState } from "react";

import { t } from "../../i18n";

export interface ConversationOutlineItem {
  id: string;
  title: string;
  previewLines: string[];
  tickWidth: number;
}

export function compactOutlineText(value: string, maxLength: number): string {
  const compact = value.trim().replace(/\s+/g, " ");
  if (compact.length <= maxLength) return compact;
  return `${compact.slice(0, Math.max(1, maxLength - 1)).trimEnd()}…`;
}

export function conversationOutlinePreview(value: string): string[] {
  const lines = value
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(
      (line) =>
        line.length > 0 &&
        !/^```/.test(line) &&
        !/^<\/?(?:details|summary)>/i.test(line),
    )
    .map((line) =>
      compactOutlineText(
        line
          .replace(/^#{1,6}\s+/, "")
          .replace(/^[-*+]\s+/, "• ")
          .replace(/^>\s?/, ""),
        88,
      ),
    );

  return lines.slice(0, 3);
}

export function outlineTickWidth(messageLength: number): number {
  if (messageLength <= 0) return 6;
  return Math.min(15, 6 + Math.round(Math.log2(messageLength + 1) * 0.85));
}

export const ConversationOutline = memo(function ConversationOutline({
  items,
  activeTurnId,
  hidden,
  onSelect,
}: {
  items: ConversationOutlineItem[];
  activeTurnId?: string;
  hidden: boolean;
  onSelect: (turnId: string) => void;
}) {
  const [previewTurnId, setPreviewTurnId] = useState<string>();
  if (hidden || items.length < 2) return null;
  const previewItem = items.find((item) => item.id === previewTurnId);

  return (
    <nav
      className="conversation-outline"
      aria-label={t("outline.ariaLabel")}
      onMouseLeave={() => setPreviewTurnId(undefined)}
    >
      <div className="conversation-outline-scroll">
        {items.map((item, index) => {
          const active = activeTurnId === item.id;
          return (
            <button
              key={item.id}
              type="button"
              className={`conversation-outline-row${active ? " active" : ""}`}
              aria-label={t("outline.turnLabel", { index: index + 1, title: item.title })}
              aria-current={active ? "true" : undefined}
              style={
                {
                  "--outline-tick-width": `${item.tickWidth}px`,
                } as CSSProperties
              }
              onClick={() => onSelect(item.id)}
              onMouseEnter={() => setPreviewTurnId(item.id)}
              onFocus={() => setPreviewTurnId(item.id)}
              onBlur={() => setPreviewTurnId(undefined)}
            >
              <span className="conversation-outline-tick" />
            </button>
          );
        })}
      </div>
      <span
        className={`conversation-outline-card${previewItem ? " visible" : ""}`}
        aria-hidden="true"
      >
        {previewItem && (
          <>
            <strong>{previewItem.title}</strong>
            {previewItem.previewLines.length > 0 ? (
              <span className="conversation-outline-preview">
                {previewItem.previewLines.map((line, lineIndex) => (
                  <span key={`${previewItem.id}-${lineIndex}`}>{line}</span>
                ))}
              </span>
            ) : (
              <span className="conversation-outline-empty">
                {t("outline.emptyPreview")}
              </span>
            )}
          </>
        )}
      </span>
    </nav>
  );
});
