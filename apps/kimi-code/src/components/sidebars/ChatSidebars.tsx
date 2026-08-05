import { Fragment, useEffect, useRef } from "react";
import {
  ArrowUp,
  BrainCircuit,
  FileCode2,
  MessageSquareText,
  Package,
  TerminalSquare,
  X,
} from "lucide-react";

import {
  isTurnRunning,
  liveStepKey,
  type InFlightTurn,
} from "../../chat/liveTurns";
import { localeTag, t } from "../../i18n";
import type { PluginCommandDetail } from "../../pluginCommandMessage";
import type { SkillDescriptor } from "../../types";
import {
  LiveAssistantContent,
  LiveThinkingBlock,
} from "../chat/ConversationViews";
import {
  MarkdownMessage,
  StreamingMarkdownMessage,
} from "../chat/MarkdownMessage";

export interface SkillDetailTarget {
  name: string;
  description?: string;
  source?: SkillDescriptor["source"];
}

export interface CompactionSummaryDetail {
  id: string;
  content: string;
  createdAt: string;
}

export function PluginCommandDetailSidebar({
  command,
  onClose,
}: {
  command: PluginCommandDetail;
  onClose: () => void;
}) {
  const commandLabel = `/${command.pluginId}:${command.commandName}`;
  return (
    <aside
      className="skill-detail-sidebar plugin-command-detail-sidebar"
      aria-label={t("plugins.commandDetailAria", { command: commandLabel })}
    >
      <header className="skill-detail-header">
        <div className="skill-detail-heading">
          <span className="skill-detail-icon">
            <TerminalSquare size={16} />
          </span>
          <div>
            <h2>{commandLabel}</h2>
            <span>{t("plugins.commandDetailSubtitle")}</span>
          </div>
        </div>
        <button
          className="icon-button quiet"
          type="button"
          aria-label={t("plugins.closeCommandDetail")}
          title={t("window.close")}
          onClick={onClose}
        >
          <X size={16} />
        </button>
      </header>

      {command.args && (
        <p className="skill-detail-description plugin-command-detail-args">
          <strong>{t("plugins.commandArgs")}</strong>
          <span>{command.args}</span>
        </p>
      )}

      <div className="skill-detail-content">
        {command.content ? (
          <div className="markdown-body skill-detail-markdown">
            <MarkdownMessage content={command.content} />
          </div>
        ) : (
          <div className="skill-detail-status">
            {t("plugins.commandDetailEmpty")}
          </div>
        )}
      </div>

      <footer className="skill-detail-path">
        <TerminalSquare size={12} />
        <span>
          {t("plugins.commandSentAt", {
            time: new Date(command.createdAt).toLocaleString(localeTag()),
          })}
        </span>
      </footer>
    </aside>
  );
}

export interface SideChatState {
  instanceId: number;
  parentSessionId: string;
  agentId?: string;
  draft: string;
  turns: InFlightTurn[];
  starting: boolean;
}

function SideChatTurnView({ turn }: { turn: InFlightTurn }) {
  const running = isTurnRunning(turn);
  const hasAssistantContent = turn.steps.some(
    (step) => step.blocks.length > 0,
  );

  return (
    <section className="side-chat-turn">
      <article className="side-chat-message user">
        <div>{turn.prompt}</div>
      </article>
      <article className={`side-chat-message assistant ${turn.status}`}>
        {turn.steps.map((step) => (
          <Fragment key={liveStepKey(step.step, step.stepId)}>
            {step.blocks.map((block, index) => {
              if (block.kind === "text") {
                return (
                  <div
                    className="markdown-body side-chat-markdown"
                    key={`text-${index}`}
                  >
                    <StreamingMarkdownMessage
                      active={running && step.status === "running"}
                      content={block.content}
                    />
                  </div>
                );
              }
              if (block.kind === "thinking") {
                return (
                  <LiveThinkingBlock
                    content={block.content}
                    key={`thinking-${index}`}
                  />
                );
              }
              if (block.kind === "content") {
                return (
                  <LiveAssistantContent
                    active={running && step.status === "running"}
                    content={block.content}
                    key={`content-${index}`}
                  />
                );
              }
              return (
                <div className="side-chat-readonly-note" key={block.toolCallId}>
                  {t("sideChat.readonlyNote")}
                </div>
              );
            })}
          </Fragment>
        ))}
        {!hasAssistantContent && running && (
          <div className="typing" aria-label={t("assistant.thinking")}>
            <i />
            <i />
            <i />
          </div>
        )}
        {turn.error && <div className="live-turn-error">{turn.error}</div>}
      </article>
    </section>
  );
}

export function SideChatSidebar({
  state,
  onDraftChange,
  onSend,
  onClose,
}: {
  state: SideChatState;
  onDraftChange: (value: string) => void;
  onSend: () => void;
  onClose: () => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const lastTurn = state.turns.at(-1);
  const sending = state.starting || isTurnRunning(lastTurn);
  const canSend = state.draft.trim().length > 0 && !sending;

  useEffect(() => {
    inputRef.current?.focus();
  }, [state.instanceId]);

  useEffect(() => {
    const element = scrollRef.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, [state.turns, state.starting]);

  return (
    <aside
      className="skill-detail-sidebar side-chat-sidebar"
      aria-label={t("sideChat.title")}
    >
      <header className="skill-detail-header">
        <div className="skill-detail-heading">
          <span className="skill-detail-icon">
            <MessageSquareText size={16} />
          </span>
          <div>
            <h2>{t("sideChat.title")}</h2>
            <span>{t("sideChat.subtitle")}</span>
          </div>
        </div>
        <button
          className="icon-button quiet"
          type="button"
          aria-label={t("sideChat.close")}
          title={t("window.close")}
          onClick={onClose}
        >
          <X size={16} />
        </button>
      </header>

      <div className="side-chat-messages" ref={scrollRef}>
        {state.turns.length === 0 ? (
          <div className="side-chat-empty">
            <span>
              <MessageSquareText size={19} />
            </span>
            <strong>{t("sideChat.emptyTitle")}</strong>
            <p>{t("sideChat.emptyCopy")}</p>
          </div>
        ) : (
          state.turns.map((turn) => (
            <SideChatTurnView
              turn={turn}
              key={`${turn.createdAt}-${turn.prompt}`}
            />
          ))
        )}
      </div>

      <form
        className="side-chat-composer"
        onSubmit={(event) => {
          event.preventDefault();
          onSend();
        }}
      >
        <textarea
          ref={inputRef}
          value={state.draft}
          rows={2}
          placeholder={t("sideChat.placeholder")}
          aria-label={t("sideChat.inputAria")}
          onChange={(event) => onDraftChange(event.target.value)}
          onKeyDown={(event) => {
            if (
              event.key === "Enter" &&
              !event.shiftKey &&
              !event.nativeEvent.isComposing
            ) {
              event.preventDefault();
              if (canSend) onSend();
            }
          }}
        />
        <button
          type="submit"
          disabled={!canSend}
          aria-label={t("sideChat.sendAria")}
          title={t("composer.send")}
        >
          {sending ? <span className="spinner light" /> : <ArrowUp size={16} />}
        </button>
      </form>
    </aside>
  );
}

export function CompactionSummarySidebar({
  summary,
  onClose,
}: {
  summary: CompactionSummaryDetail;
  onClose: () => void;
}) {
  return (
    <aside
      className="skill-detail-sidebar compaction-summary-sidebar"
      aria-label={t("compaction.summaryAria")}
    >
      <header className="skill-detail-header">
        <div className="skill-detail-heading">
          <span className="skill-detail-icon">
            <BrainCircuit size={16} />
          </span>
          <div>
            <h2>{t("compaction.summaryTitle")}</h2>
            <span>{t("compaction.summarySubtitle")}</span>
          </div>
        </div>
        <button
          className="icon-button quiet"
          type="button"
          aria-label={t("compaction.closeSummary")}
          title={t("window.close")}
          onClick={onClose}
        >
          <X size={16} />
        </button>
      </header>

      <div className="skill-detail-content">
        {summary.content ? (
          <div className="markdown-body skill-detail-markdown">
            <MarkdownMessage content={summary.content} />
          </div>
        ) : (
          <div className="skill-detail-status">{t("compaction.summaryEmpty")}</div>
        )}
      </div>

      <footer className="skill-detail-path">
        <BrainCircuit size={12} />
        <span>
          {t("compaction.generatedAt", { time: new Date(summary.createdAt).toLocaleString(localeTag()) })}
        </span>
      </footer>
    </aside>
  );
}

function skillSourceLabel(source?: SkillDescriptor["source"]): string {
  switch (source) {
    case "project":
      return t("skills.sourceProject");
    case "user":
      return t("skills.sourceUser");
    case "extra":
      return t("skills.sourceExtra");
    case "builtin":
      return t("skills.sourceBuiltin");
    default:
      return t("skills.detailFallback");
  }
}

export function SkillDetailSidebar({
  skill,
  content,
  path,
  busy,
  error,
  onClose,
  onRetry,
}: {
  skill: SkillDetailTarget;
  content?: string;
  path?: string;
  busy: boolean;
  error?: string;
  onClose: () => void;
  onRetry: () => void;
}) {
  return (
    <aside className="skill-detail-sidebar" aria-label={t("skills.detailAria", { name: skill.name })}>
      <header className="skill-detail-header">
        <div className="skill-detail-heading">
          <span className="skill-detail-icon">
            <Package size={16} />
          </span>
          <div>
            <h2>{skill.name}</h2>
            <span>{skillSourceLabel(skill.source)}</span>
          </div>
        </div>
        <button
          className="icon-button quiet"
          type="button"
          aria-label={t("skills.closeDetail")}
          title={t("window.close")}
          onClick={onClose}
        >
          <X size={16} />
        </button>
      </header>

      {skill.description && (
        <p className="skill-detail-description">{skill.description}</p>
      )}

      <div className="skill-detail-content">
        {busy ? (
          <div className="skill-detail-status">
            <span className="spinner" />
            <span>{t("skills.loadingDetail")}</span>
          </div>
        ) : error ? (
          <div className="skill-detail-status error">
            <span>{error}</span>
            <button type="button" onClick={onRetry}>
              {t("common.retry")}
            </button>
          </div>
        ) : content ? (
          <div className="markdown-body skill-detail-markdown">
            <MarkdownMessage content={content} />
          </div>
        ) : (
          <div className="skill-detail-status">{t("skills.detailEmpty")}</div>
        )}
      </div>

      {path && (
        <footer className="skill-detail-path" title={path}>
          <FileCode2 size={12} />
          <span>{path}</span>
        </footer>
      )}
    </aside>
  );
}
