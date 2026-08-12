import {
  Fragment,
  type ReactNode,
  memo,
  useEffect,
  useState,
} from "react";
import {
  AlarmClock,
  ArrowUp,
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  Code2,
  Copy,
  CornerDownRight,
  FileCode2,
  FileDiff,
  MoreHorizontal,
  MessageSquareText,
  Square,
  TerminalSquare,
  Trash2,
  Undo2,
  Wrench,
  X,
} from "lucide-react";

import {
  finalResponseMessage,
  formatElapsedDuration,
  groupHistoryMessages,
  mergeHistoryToolResults,
  messageOriginKind,
  type HistoryConversationTurn,
  type RenderMessage,
  type ToolResultContent,
} from "../../chat/history";
import {
  isTurnRunning,
  liveStepKey,
  type InFlightTurn,
  type LiveBlock,
  type LiveSteeredPrompt,
  type PromptAttachment,
  type QueuedPrompt,
  type RemoteQueuedPrompt,
} from "../../chat/liveTurns";
import { hasVisibleLiveUserMessage } from "../../chat/liveTurnVisibility";
import { isVisibleRetryStep } from "../../chat/retryStatus";
import {
  mediaSourceUrl,
  messagePluginCommand,
  messageStructuredContent,
  messageText,
  messageThinking,
  structuredValue,
  type PluginCommandDisplay,
} from "../../chat/messages";
import { toolInputSummary } from "../../chat/toolInputSummary";
import { parseStreamingToolInput } from "../../chat/streamingToolInput";
import { parseCronFireMessage, type CronFireMessage } from "../../cronFire";
import { t } from "../../i18n";
import type { SubagentConversationHistory } from "../../app/appUtils";
import type { PluginCommandDetail } from "../../pluginCommandMessage";
import { SkillPromptDisplayContent } from "../../prompt/SkillPromptDisplay";
import {
  collectHistoricalSubagentRuns,
  subagentInvocationMessages,
  subagentRunsWithSwarmItems,
  type SubagentRun,
  type SubagentRunStatus,
  type SubagentRunsByTool,
} from "../../subagentEvents";
import type {
  AgentContentPart,
  CompactionEvent,
  MessageContent,
  Project,
  ProtocolMessage,
  TurnFileChange,
} from "../../types";
import {
  formatBytes,
  formatCompactTokenCount,
  formatTime,
  inputTokenUsage,
} from "../../utils/format";
import { Collapsible } from "../Collapsible";
import { compactionTokenTransition } from "../ChatHeader";
import { MarkdownMessage, StreamingMarkdownMessage } from "./MarkdownMessage";
import { PreviewableImage } from "./PreviewableImage";

// Evaluated at render time so the text follows language switches.
function promptSuggestions() {
  return [
    {
      icon: <FileCode2 size={17} />,
      title: t("suggestion.explore.title"),
      prompt: t("suggestion.explore.prompt"),
    },
    {
      icon: <Wrench size={17} />,
      title: t("suggestion.debug.title"),
      prompt: t("suggestion.debug.prompt"),
    },
    {
      icon: <TerminalSquare size={17} />,
      title: t("suggestion.feature.title"),
      prompt: t("suggestion.feature.prompt"),
    },
  ];
}

export function Welcome({
  project,
  onSuggestion,
}: {
  project: Project;
  onSuggestion: (value: string) => void;
}) {
  return (
    <section className="welcome">
      <div className="welcome-orbit">
        <span className="orbit orbit-one" />
        <span className="orbit orbit-two" />
        <div className="welcome-mark">
          <Code2 size={27} />
        </div>
      </div>
      <p className="eyebrow">KIMI CODE AGENT</p>
      <h2>
        {t("welcome.title")}
        <br />
        <span>{project.name}</span>
        {t("welcome.titleSuffix")}
      </h2>
      <p className="welcome-copy">
        {t("welcome.copy")}
      </p>
      <div className="suggestion-grid">
        {promptSuggestions().map((suggestion) => (
          <button
            key={suggestion.title}
            onClick={() => onSuggestion(suggestion.prompt)}
          >
            <span>{suggestion.icon}</span>
            <strong>{suggestion.title}</strong>
            <small>{suggestion.prompt}</small>
            <ArrowUp size={15} />
          </button>
        ))}
      </div>
    </section>
  );
}

export function LiveTurnView({
  turn,
  outlineId,
  subagentRuns,
  subagentLiveTurns,
  onSkillOpen,
  onPluginCommandOpen,
}: {
  turn: InFlightTurn;
  outlineId?: string;
  subagentRuns?: SubagentRunsByTool;
  subagentLiveTurns?: Record<string, InFlightTurn>;
  onSkillOpen: (name: string) => void;
  onPluginCommandOpen: (command: PluginCommandDetail) => void;
}) {
  const hasBlocks = turn.steps.some((step) => step.blocks.length > 0);
  const visibleSteps = turn.steps.filter((step) =>
    isVisibleRetryStep(step, turn.steeredPrompts),
  );
  const streaming = isTurnRunning(turn);
  const cronFire = parseCronFireMessage(turn.prompt);
  const showUserMessage = hasVisibleLiveUserMessage(turn);

  return (
    <section
      className="conversation-turn live-conversation-turn"
      data-conversation-turn-id={outlineId}
    >
      {showUserMessage && (
        <article className={`message user-message live-user-message${cronFire ? " cron-fire-message" : ""}`}>
          <div className="message-meta">
            <time>{formatTime(turn.createdAt)}</time>
          </div>
          <div className={`user-bubble${cronFire ? " cron-fire-bubble" : ""}`}>
            {cronFire ? (
              <CronFireMessageContent fire={cronFire} />
            ) : turn.pluginCommand ? (
              <PluginCommandDisplayContent
                command={turn.pluginCommand}
                onOpen={() =>
                  onPluginCommandOpen({
                    ...turn.pluginCommand!,
                    id: turn.userMessageId ?? turn.promptId ?? turn.createdAt,
                    content: turn.pluginCommandContent ?? "",
                    createdAt: turn.createdAt,
                  })
                }
              />
            ) : (
              <SkillPromptDisplayContent
                text={turn.prompt}
                skills={turn.skills}
                onSkillOpen={onSkillOpen}
              />
            )}
            <PromptAttachmentContent attachments={turn.attachments} />
          </div>
        </article>
      )}
      <article className={`message assistant-message live-turn ${turn.status}`}>
        <div className="assistant-body">
          {visibleSteps.map((step) => {
            const stepKey = liveStepKey(step.step, step.stepId);
            const steeredPrompts = turn.steeredPrompts.filter(
              (item) => item.anchorStepKey === stepKey,
            );
            return (
              <section
                className={`live-step ${step.status}`}
                key={stepKey}
              >
                {steeredPrompts
                  .filter((item) => item.afterBlockIndex === -1)
                  .map((item) => (
                    <LiveSteeredPromptView
                      item={item}
                      onSkillOpen={onSkillOpen}
                      key={item.promptId}
                    />
                  ))}
                {step.blocks.map((block, index) => {
                  let blockView: ReactNode;
                  if (block.kind === "text") {
                    blockView = (
                      <div className="markdown-body live-text">
                        <StreamingMarkdownMessage
                          active={streaming && step.status === "running"}
                          content={block.content}
                        />
                      </div>
                    );
                  } else if (block.kind === "thinking") {
                    blockView = (
                      <LiveThinkingBlock content={block.content} />
                    );
                  } else if (block.kind === "content") {
                    blockView = (
                      <LiveAssistantContent
                        active={streaming && step.status === "running"}
                        content={block.content}
                      />
                    );
                  } else {
                    blockView = (
                      <LiveToolBlock
                        tool={block}
                        subagents={subagentRuns?.[block.toolCallId] ?? []}
                        subagentRuns={subagentRuns}
                        subagentLiveTurns={subagentLiveTurns}
                      />
                    );
                  }
                  const blockKey =
                    block.kind === "tool"
                      ? block.toolCallId
                      : `${block.kind}-${index}`;
                  return (
                    <Fragment key={blockKey}>
                      {blockView}
                      {steeredPrompts
                        .filter((item) => item.afterBlockIndex === index)
                        .map((item) => (
                          <LiveSteeredPromptView
                            item={item}
                            onSkillOpen={onSkillOpen}
                            key={item.promptId}
                          />
                        ))}
                    </Fragment>
                  );
                })}
                {step.interruption && (
                  <div className="live-step-interruption">
                    {step.interruption}
                  </div>
                )}
              </section>
            );
          })}
          {turn.steeredPrompts
            .filter((item) => item.anchorStepKey === undefined)
            .map((item) => (
              <LiveSteeredPromptView
                item={item}
                onSkillOpen={onSkillOpen}
                key={item.promptId}
              />
            ))}
          {!hasBlocks &&
            (turn.status === "queued" || turn.status === "running") && (
              <div className="typing">
                <i />
                <i />
                <i />
              </div>
            )}
          {turn.error && <div className="live-turn-error">{turn.error}</div>}
          {turn.fileChanges && turn.fileChanges.length > 0 && (
            <TurnFileChangesCard files={turn.fileChanges} />
          )}
          <AssistantResponseStatus
            running={isTurnRunning(turn)}
            durationMs={turn.durationMs}
            retryAttempt={turn.retry?.attempt}
          />
        </div>
      </article>
    </section>
  );
}

function PluginCommandDisplayContent({
  command,
  onOpen,
}: {
  command: PluginCommandDisplay;
  onOpen?: () => void;
}) {
  const content = (
    <>
      <div className="plugin-command-message-heading">
        <TerminalSquare size={14} aria-hidden="true" />
        <span>/{command.pluginId}:{command.commandName}</span>
      </div>
      {command.args && (
        <div className="plugin-command-message-args">{command.args}</div>
      )}
    </>
  );
  return onOpen ? (
    <button
      className="plugin-command-message"
      type="button"
      title={t("plugins.openCommandDetail")}
      onClick={onOpen}
    >
      {content}
    </button>
  ) : (
    <div className="plugin-command-message">{content}</div>
  );
}

function CronFireMessageContent({ fire }: { fire: CronFireMessage }) {
  return (
    <div className="cron-fire-content">
      <div className="cron-fire-summary">
        <span className="cron-fire-icon" aria-hidden="true">
          <AlarmClock size={18} />
        </span>
        <div>
          <strong>{t("cron.fireTitle")}</strong>
          <span>
            {fire.recurring ? t("cron.recurring") : t("cron.once")}
            <code>{fire.cron}</code>
          </span>
        </div>
      </div>
      <p>{fire.prompt}</p>
      {(fire.coalescedCount > 1 || fire.stale) && (
        <div className="cron-fire-flags">
          {fire.coalescedCount > 1 && (
            <span>{t("cron.fireCoalesced", { count: fire.coalescedCount })}</span>
          )}
          {fire.stale && <span>{t("cron.fireFinal")}</span>}
        </div>
      )}
    </div>
  );
}

export function LiveSteeredPromptView({
  item,
  onSkillOpen,
}: {
  item: LiveSteeredPrompt;
  onSkillOpen: (name: string) => void;
}) {
  const message = item.message;
  if (!message) return null;
  return (
    <div className="live-steered-message user-message">
      <div className="message-meta">
        <time>{formatTime(message.createdAt)}</time>
      </div>
      <div className="user-bubble">
        <SkillPromptDisplayContent
          text={message.text}
          skills={message.skills}
          onSkillOpen={onSkillOpen}
        />
        <PromptAttachmentContent attachments={message.attachments} />
      </div>
    </div>
  );
}

export function QueuedPromptList({
  prompts,
  remotePrompts,
  canSteer,
  onRemove,
  onSteer,
  onSkillOpen,
}: {
  prompts: readonly QueuedPrompt[];
  remotePrompts: readonly RemoteQueuedPrompt[];
  canSteer: boolean;
  onRemove: (queuedPromptId: string) => void;
  onSteer: (queuedPromptId: string) => void;
  onSkillOpen: (name: string) => void;
}) {
  const count = prompts.length + remotePrompts.length;
  return (
    <section className="queued-prompt-stack" aria-label={t("queue.ariaLabel")}>
      <header>
        <span>
          <MessageSquareText size={13} />
          {t("queue.title", { count })}
        </span>
        <small>{t("queue.hint")}</small>
      </header>
      <div className="queued-prompt-list">
        {prompts.map((prompt) => (
          <article
            className={`queued-prompt ${prompt.executionState ?? "pending"}`}
            key={prompt.id}
          >
            <CornerDownRight className="queued-prompt-leading" size={15} />
            <div className="queued-prompt-content">
              {prompt.text || prompt.skills.length > 0 ? (
                <div>
                  <SkillPromptDisplayContent
                    text={prompt.text}
                    skills={prompt.skills.map((skill) => skill.name)}
                    onSkillOpen={onSkillOpen}
                  />
                </div>
              ) : (
                <span className="queued-prompt-placeholder">
                  {t("queue.attachmentsOnly", {
                    count: prompt.attachments.length,
                  })}
                </span>
              )}
              <PromptAttachmentContent attachments={prompt.attachments} />
            </div>
            <div className="queued-prompt-actions">
              {prompt.executionState ? (
                <span className="queued-prompt-state" aria-live="polite">
                  {prompt.executionState === "submitting" && (
                    <span className="spinner" />
                  )}
                  {prompt.executionState === "submitting"
                    ? t("queue.submitting")
                    : t("queue.waitingExecution")}
                </span>
              ) : (
                <button
                  className="queued-prompt-steer"
                  type="button"
                  disabled={
                    !canSteer ||
                    prompt.skills.length > 0 ||
                    prompt.goalMode
                  }
                  title={
                    prompt.goalMode
                      ? t("queue.goalPending")
                      : prompt.skills.length > 0
                        ? t("queue.skillPending")
                        : canSteer
                          ? t("queue.steer")
                          : t("queue.steerPending")
                  }
                  aria-label={t("queue.steerAria")}
                  onClick={() => onSteer(prompt.id)}
                >
                  <CornerDownRight size={13} />
                  {t("queue.steer")}
                </button>
              )}
              <button
                className="queued-prompt-remove"
                type="button"
                disabled={prompt.executionState !== undefined}
                title={t("queue.withdraw")}
                aria-label={t("queue.withdrawAria")}
                onClick={() => onRemove(prompt.id)}
              >
                <Trash2 size={14} />
              </button>
            </div>
          </article>
        ))}
        {remotePrompts.map((prompt) => (
          <article
            className="queued-prompt waiting remote"
            key={prompt.promptId}
          >
            <CornerDownRight className="queued-prompt-leading" size={15} />
            <div className="queued-prompt-content">
              {prompt.text || prompt.skills.length > 0 ? (
                <div>
                  <SkillPromptDisplayContent
                    text={prompt.text}
                    skills={prompt.skills}
                    onSkillOpen={onSkillOpen}
                  />
                </div>
              ) : (
                <span className="queued-prompt-placeholder">
                  {t("queue.attachmentsOnly", {
                    count: prompt.attachments.length,
                  })}
                </span>
              )}
              <PromptAttachmentContent attachments={prompt.attachments} />
            </div>
            <div className="queued-prompt-actions">
              <span className="queued-prompt-state">
                {t("queue.waitingExecution")}
              </span>
            </div>
          </article>
        ))}
      </div>
    </section>
  );
}

export function AssistantResponseStatus({
  running,
  durationMs,
  retryAttempt,
}: {
  running: boolean;
  durationMs?: number;
  retryAttempt?: number;
}) {
  if (!running && durationMs === undefined) return null;
  return (
    <div className="assistant-response-status-group" aria-live="polite">
      <div
        className={`assistant-response-status ${running ? "thinking" : "elapsed"}`}
      >
        {running ? (
          <>
            <span>{t("assistant.thinking")}</span>
            <span className="assistant-thinking-dots" aria-hidden="true">
              <i />
              <i />
              <i />
            </span>
          </>
        ) : (
          <span>{t("assistant.elapsed", { duration: formatElapsedDuration(durationMs ?? 0) })}</span>
        )}
      </div>
      {running && retryAttempt !== undefined && (
        <div className="assistant-retry-status">
          {t("assistant.retrying", { attempt: retryAttempt })}
        </div>
      )}
    </div>
  );
}

function lastThinkingSentence(content: string): string {
  const normalized = content
    .replace(/[^\S\r\n]+/g, " ")
    .replace(/\r\n?/g, "\n")
    .trim();
  if (!normalized) return "";
  const sentences = normalized
    .split(/\n+|(?<=[。！？!?])[^\S\r\n]*|(?<=\.)[^\S\r\n]+(?=[A-Z\u3400-\u9fff])/u)
    .map((sentence) => sentence.trim())
    .filter(Boolean);
  return sentences.at(-1) ?? normalized;
}

function ThinkingSummary({ content }: { content: string }) {
  const [open, setOpen] = useState(false);
  const summary = lastThinkingSentence(content);
  if (!summary) return null;
  return (
    <div className="thinking-summary-block">
      <button
        type="button"
        className="thinking-summary-toggle"
        aria-expanded={open}
        title={open ? t("thinking.collapse") : t("thinking.expand")}
        onClick={() => setOpen((value) => !value)}
      >
        <span>{summary}</span>
      </button>
      <Collapsible open={open}>
        <p className="thinking-full">{content}</p>
      </Collapsible>
    </div>
  );
}

export function LiveThinkingBlock({ content }: { content: string }) {
  return <ThinkingSummary content={content} />;
}

export function LiveAssistantContent({
  content,
  active,
}: {
  content: AgentContentPart;
  active: boolean;
}) {
  switch (content.type) {
    case "text":
      return (
        <div className="markdown-body live-text">
          <StreamingMarkdownMessage active={active} content={content.text} />
        </div>
      );
    case "think":
      return <LiveThinkingBlock content={content.think} />;
    case "image_url":
      return (
        <MessageImage
          src={content.imageUrl.url}
          alt={t("message.imageAlt")}
          path={content.imageUrl.id}
        />
      );
    case "audio_url":
      return <MessageAudio src={content.audioUrl.url} />;
    case "video_url":
      return <MessageVideo src={content.videoUrl.url} />;
  }
}

export function LiveToolBlock({
  tool,
  subagents,
  subagentRuns,
  subagentLiveTurns,
}: {
  tool: Extract<LiveBlock, { kind: "tool" }>;
  subagents: readonly SubagentRun[];
  subagentRuns?: SubagentRunsByTool;
  subagentLiveTurns?: Record<string, InFlightTurn>;
}) {
  const active = tool.status === "streaming" || tool.status === "running";
  const [open, setOpen] = useState(false);
  const progress = tool.updates.at(-1);
  const updateLog = tool.updates
    .filter((update) => update.text)
    .slice(-20)
    .map((update) => update.text)
    .join("\n");
  const input =
    tool.input ??
    (tool.argumentsText
      ? parseStructuredValue(tool.argumentsText)
      : undefined);
  const streamingInput =
    tool.input === undefined && tool.argumentsText
      ? parseStreamingToolInput(tool.argumentsText)
      : undefined;
  const summary = toolInputSummary(input);
  const displayedSubagents = subagentRunsWithSwarmItems(subagents, input);
  return (
    <div className={`live-tool-card ${tool.status}`}>
      <button
        type="button"
        className="tool-card-summary"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <ToolStatusIcon status={tool.status} />
        <Wrench size={13} />
        <span className="tool-card-name">{tool.name ?? t("tool.preparing")}</span>
        {summary && (
          <span className="tool-card-preview" title={summary}>
            {summary}
          </span>
        )}
        <small>{liveToolStatusLabel(tool.status)}</small>
      </button>
      {displayedSubagents.length > 0 && (
        <SubagentPanel
          subagents={displayedSubagents}
          liveTurns={subagentLiveTurns}
          nestedRuns={subagentRuns}
          parentActive={active}
        />
      )}
      <Collapsible className="tool-card-collapse" open={open}>
        <div className="live-tool-detail">
          {tool.description && <p>{tool.description}</p>}
          {input !== undefined && (
            <section className="tool-detail-section">
              <span>{t("tool.params")}</span>
              <ToolInputView
                name={tool.name}
                input={input}
                streamingInput={streamingInput}
              />
            </section>
          )}
          {updateLog && <pre className="live-tool-update-log">{updateLog}</pre>}
          {progress && (progress.percent !== undefined || !updateLog) && (
            <div className="live-tool-progress">
              <span>{progress.text ?? progress.kind}</span>
              {progress.percent !== undefined && (
                <strong>{Math.round(progress.percent)}%</strong>
              )}
            </div>
          )}
          {tool.output !== undefined && (
            <ToolResultView
              name={tool.name}
              output={tool.output}
              isError={tool.isError}
            />
          )}
        </div>
      </Collapsible>
    </div>
  );
}

type DisplaySubagentStatus = SubagentRunStatus | "stopped";

function displayedSubagentStatus(
  subagent: SubagentRun,
  parentActive: boolean,
): DisplaySubagentStatus {
  if (
    !parentActive &&
    (subagent.status === "queued" ||
      subagent.status === "running" ||
      subagent.status === "suspended")
  ) {
    return "stopped";
  }
  return subagent.status;
}

function subagentStatusLabel(status: DisplaySubagentStatus): string {
  switch (status) {
    case "queued":
      return t("status.queued");
    case "running":
      return t("status.executing");
    case "suspended":
      return t("status.suspended");
    case "completed":
      return t("status.completed");
    case "failed":
      return t("status.failed");
    case "stopped":
      return t("status.stopped");
  }
}

function subagentPanelSummary(statuses: DisplaySubagentStatus[]): string {
  const running = statuses.filter((status) => status === "running").length;
  const suspended = statuses.filter(
    (status) => status === "suspended",
  ).length;
  const queued = statuses.filter((status) => status === "queued").length;
  const failed = statuses.filter((status) => status === "failed").length;
  if (running > 0) return t("subagent.runningCount", { count: running });
  if (suspended > 0) return t("subagent.suspendedCount", { count: suspended });
  if (queued > 0) return t("subagent.queuedCount", { count: queued });
  if (failed > 0) return t("subagent.failedCount", { count: failed });
  if (statuses.some((status) => status === "stopped")) return t("status.stopped");
  return t("subagent.allDone");
}

function SubagentStatusIcon({ status }: { status: DisplaySubagentStatus }) {
  return (
    <span
      className={`subagent-status-icon ${status}`}
      aria-label={subagentStatusLabel(status)}
    >
      {status === "completed" ? (
        <Check size={10} />
      ) : status === "failed" ? (
        <X size={10} />
      ) : status === "suspended" ? (
        <MoreHorizontal size={10} />
      ) : status === "stopped" ? (
        <Square size={7} />
      ) : null}
    </span>
  );
}

function SubagentPanel({
  subagents,
  liveTurns,
  nestedRuns,
  histories,
  onLoadHistory,
  parentActive,
}: {
  subagents: readonly SubagentRun[];
  liveTurns?: Record<string, InFlightTurn>;
  nestedRuns?: SubagentRunsByTool;
  histories?: Record<string, SubagentConversationHistory>;
  onLoadHistory?: (agentId: string, force?: boolean) => void;
  parentActive: boolean;
}) {
  const statuses = subagents.map((subagent) =>
    displayedSubagentStatus(subagent, parentActive),
  );
  const active = statuses.some(
    (status) =>
      status === "queued" ||
      status === "running" ||
      status === "suspended",
  );
  const finished = statuses.filter(
    (status) =>
      status === "completed" ||
      status === "failed" ||
      status === "stopped",
  ).length;
  const [open, setOpen] = useState(false);

  return (
    <section
      className={`subagent-panel ${active ? "active" : "settled"}`}
      aria-label={t("subagent.progressAria")}
    >
      <button
        type="button"
        className="subagent-panel-summary"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <Bot size={13} />
        <span>{t("subagent.title")}</span>
        <strong>
          {finished}/{subagents.length}
        </strong>
        <span className="subagent-progress-dots" aria-hidden="true">
          {statuses.map((status, index) => (
            <i className={status} key={`${status}-${index}`} />
          ))}
        </span>
        <small>{subagentPanelSummary(statuses)}</small>
        {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
      </button>
      <Collapsible className="subagent-list-collapse" open={open}>
        <div
          className="subagent-list"
          aria-live="polite"
        >
          {subagents.map((subagent, index) => (
            <SubagentRow
              key={subagent.subagentId}
              subagent={subagent}
              status={statuses[index]}
              liveTurn={liveTurns?.[subagent.subagentId]}
              liveTurns={liveTurns}
              nestedRuns={nestedRuns}
              history={histories?.[subagent.subagentId]}
              histories={histories}
              onLoadHistory={onLoadHistory}
            />
          ))}
        </div>
      </Collapsible>
    </section>
  );
}

function SubagentRow({
  subagent,
  status,
  liveTurn,
  liveTurns,
  nestedRuns,
  history,
  histories,
  onLoadHistory,
}: {
  subagent: SubagentRun;
  status: DisplaySubagentStatus;
  liveTurn?: InFlightTurn;
  liveTurns?: Record<string, InFlightTurn>;
  nestedRuns?: SubagentRunsByTool;
  history?: SubagentConversationHistory;
  histories?: Record<string, SubagentConversationHistory>;
  onLoadHistory?: (agentId: string, force?: boolean) => void;
}) {
  const hasDetail =
    liveTurn !== undefined ||
    (onLoadHistory !== undefined && runHasHistory(subagent)) ||
    Boolean(subagent.resultSummary) ||
    Boolean(subagent.error) ||
    subagent.usage !== undefined ||
    subagent.contextTokens !== undefined;
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (open && history === undefined && onLoadHistory && runHasHistory(subagent)) {
      onLoadHistory(subagent.subagentId);
    }
  }, [
    history,
    onLoadHistory,
    open,
    subagent.historyAvailable,
    subagent.historyPrompt,
    subagent.subagentId,
  ]);
  const tokenTotal = subagent.usage
    ? inputTokenUsage(subagent.usage) + subagent.usage.output
    : undefined;
  const activity = subagentLiveActivity(liveTurn);
  const shortId =
    subagent.subagentId.length > 18
      ? `${subagent.subagentId.slice(0, 8)}…${subagent.subagentId.slice(-5)}`
      : subagent.subagentId;

  return (
    <div className={`subagent-row ${status}`}>
      <button
        type="button"
        className="subagent-row-summary"
        aria-expanded={hasDetail ? open : undefined}
        disabled={!hasDetail}
        onClick={() => {
          if (!hasDetail) return;
          setOpen((value) => !value);
        }}
      >
        <SubagentStatusIcon status={status} />
        <span className="subagent-row-copy">
          <strong>
            {subagent.description ||
              t("subagent.fallbackName", { name: subagent.swarmIndex ?? subagent.subagentName })}
          </strong>
          <small>
            {subagent.swarmIndex !== undefined &&
              `#${subagent.swarmIndex} · `}
            {subagent.subagentName} ·{" "}
            <span title={subagent.subagentId}>{shortId}</span>
            {subagent.runInBackground && t("subagent.backgroundSuffix")}
          </small>
          {activity && <span className="subagent-row-activity">{activity}</span>}
        </span>
        <span className={`subagent-row-state ${status}`}>
          {subagentStatusLabel(status)}
        </span>
        {hasDetail &&
          (open ? <ChevronDown size={11} /> : <ChevronRight size={11} />)}
      </button>
      <Collapsible className="subagent-row-collapse" open={open && hasDetail}>
        <div className="subagent-row-detail">
          {liveTurn && (
            <SubagentLiveTimeline
              turn={liveTurn}
              liveTurns={liveTurns}
              nestedRuns={nestedRuns}
            />
          )}
          {!liveTurn && history?.loading && (
            <div className="subagent-history-state">
              <span className="spinner" />
              {t("subagent.historyLoading")}
            </div>
          )}
          {!liveTurn && history?.error && (
            <div className="subagent-history-state error">
              <span>{history.error}</span>
              <button
                type="button"
                onClick={() => onLoadHistory?.(subagent.subagentId, true)}
              >
                {t("common.retry")}
              </button>
            </div>
          )}
          {!liveTurn && history && !history.loading && !history.error && (
            <SubagentHistoryTimeline
              history={history}
              run={subagent}
              histories={histories}
              onLoadHistory={onLoadHistory}
            />
          )}
          {subagent.resultSummary && (
            <section className="subagent-result-summary">
              <header>
                <span>{t("subagent.finalSummary")}</span>
                <AgentToolCopyButton text={subagent.resultSummary} />
              </header>
              <div className="subagent-result-markdown markdown-body">
                <MarkdownMessage content={subagent.resultSummary} />
              </div>
            </section>
          )}
          {subagent.error && (
            <section className="subagent-result-summary">
              <span>{status === "failed" ? t("subagent.errorLabel") : t("subagent.statusNote")}</span>
              <pre className={status === "failed" ? "error" : ""}>
                {subagent.error}
              </pre>
            </section>
          )}
          {(tokenTotal !== undefined ||
            subagent.contextTokens !== undefined) && (
            <div className="subagent-metrics">
              {tokenTotal !== undefined && (
                <span>Token {formatCompactTokenCount(tokenTotal)}</span>
              )}
              {subagent.contextTokens !== undefined && (
                <span>
                  {t("subagent.contextTokens", { count: formatCompactTokenCount(subagent.contextTokens) })}
                </span>
              )}
            </div>
          )}
        </div>
      </Collapsible>
    </div>
  );
}

function runHasHistory(run: SubagentRun): boolean {
  return run.historyAvailable === true && Boolean(run.historyPrompt);
}

function SubagentHistoryTimeline({
  history,
  run,
  histories,
  onLoadHistory,
}: {
  history: SubagentConversationHistory;
  run: SubagentRun;
  histories?: Record<string, SubagentConversationHistory>;
  onLoadHistory?: (agentId: string, force?: boolean) => void;
}) {
  const messages = subagentInvocationMessages(history.items, run);
  if (messages.length === 0) {
    return (
      <div className="subagent-history-state">
        {t("subagent.historyEmpty")}
      </div>
    );
  }
  const presentation = mergeHistoryToolResults(messages);
  const turns = groupHistoryMessages(presentation.messages);
  const nestedRuns = collectHistoricalSubagentRuns(messages);

  return (
    <div className="subagent-live-timeline historical">
      {turns.flatMap((turn) =>
        turn.responses.map((message) => (
          <AssistantMessagePart
            key={message.id}
            message={message}
            toolResults={presentation.results}
            subagentRuns={nestedRuns}
            subagentHistories={histories}
            onLoadSubagentHistory={onLoadHistory}
            onCompactionSummaryOpen={() => undefined}
          />
        )),
      )}
    </div>
  );
}

function subagentLiveActivity(turn?: InFlightTurn): string | undefined {
  if (!turn) return undefined;
  for (let stepIndex = turn.steps.length - 1; stepIndex >= 0; stepIndex -= 1) {
    const blocks = turn.steps[stepIndex].blocks;
    for (let blockIndex = blocks.length - 1; blockIndex >= 0; blockIndex -= 1) {
      const block = blocks[blockIndex];
      if (block.kind === "tool") {
        return block.status === "running" || block.status === "streaming"
          ? t("subagent.executingTool", { name: block.name ?? t("tool.fallback") })
          : t("subagent.toolEnded", { name: block.name ?? t("tool.fallback") });
      }
      if (block.kind === "thinking") return t("assistant.thinking");
      if (
        block.kind === "text" ||
        (block.kind === "content" && block.content.type === "text")
      ) {
        return isTurnRunning(turn) ? t("subagent.generating") : t("subagent.responseReady");
      }
    }
  }
  return isTurnRunning(turn) ? t("subagent.starting") : t("subagent.taskEnded");
}

function SubagentLiveTimeline({
  turn,
  liveTurns,
  nestedRuns,
}: {
  turn: InFlightTurn;
  liveTurns?: Record<string, InFlightTurn>;
  nestedRuns?: SubagentRunsByTool;
}) {
  const streaming = isTurnRunning(turn);
  const hasBlocks = turn.steps.some((step) => step.blocks.length > 0);

  return (
    <div className="subagent-live-timeline">
      {turn.steps.map((step) => (
        <section
          className={`subagent-live-step ${step.status}`}
          key={step.stepId ?? step.step}
        >
          {step.blocks.map((block, index) => {
            if (block.kind === "text") {
              return (
                <div
                  className="markdown-body live-text"
                  key={`${block.kind}-${index}`}
                >
                  <StreamingMarkdownMessage
                    active={streaming && step.status === "running"}
                    content={block.content}
                  />
                </div>
              );
            }
            if (block.kind === "thinking") {
              return (
                <LiveThinkingBlock
                  content={block.content}
                  key={`${block.kind}-${index}`}
                />
              );
            }
            if (block.kind === "content") {
              return (
                <LiveAssistantContent
                  active={streaming && step.status === "running"}
                  content={block.content}
                  key={`${block.kind}-${index}`}
                />
              );
            }
            return (
              <LiveToolBlock
                tool={block}
                subagents={nestedRuns?.[block.toolCallId] ?? []}
                subagentRuns={nestedRuns}
                subagentLiveTurns={liveTurns}
                key={block.toolCallId}
              />
            );
          })}
          {step.interruption && (
            <div className="live-step-interruption">{step.interruption}</div>
          )}
        </section>
      ))}
      {!hasBlocks && streaming && (
        <div className="subagent-live-placeholder">
          <span className="spinner" />
          {t("subagent.waitingOutput")}
        </div>
      )}
      {turn.error && <div className="live-turn-error">{turn.error}</div>}
    </div>
  );
}

function parseStructuredValue(value: string): unknown {
  try {
    return JSON.parse(value);
  } catch {
    return value;
  }
}

function liveToolStatusLabel(
  status: Extract<LiveBlock, { kind: "tool" }>["status"],
): string {
  switch (status) {
    case "streaming":
      return t("status.preparing");
    case "running":
      return t("status.executing");
    case "completed":
      return t("status.completed");
    case "error":
      return t("status.failed");
  }
}

function ToolStatusIcon({
  status,
}: {
  status:
    | Extract<LiveBlock, { kind: "tool" }>["status"]
    | "incomplete";
}) {
  if (status === "streaming" || status === "running") {
    return <span className="tool-status-icon spinning" aria-label={t("status.executing")} />;
  }
  if (status === "completed") {
    return (
      <span className="tool-status-icon completed" aria-label={t("status.completed")}>
        <Check size={11} />
      </span>
    );
  }
  if (status === "error") {
    return (
      <span className="tool-status-icon error" aria-label={t("status.error")}>
        <X size={11} />
      </span>
    );
  }
  return (
    <span className="tool-status-icon incomplete" aria-label={t("status.incomplete")}>
      <MoreHorizontal size={11} />
    </span>
  );
}

export const HistoryTurnView = memo(function HistoryTurnView({
  turn,
  toolResults,
  subagentRuns,
  subagentHistories,
  onLoadSubagentHistory,
  messageDurations,
  messageFileChanges,
  undoableUserMessageId,
  onUndoUserMessage,
  copiedMessageId,
  onCopy,
  onSkillOpen,
  onPluginCommandOpen,
  onCompactionSummaryOpen,
  compactionEvent,
}: {
  turn: HistoryConversationTurn;
  toolResults: Map<string, ToolResultContent>;
  subagentRuns?: SubagentRunsByTool;
  subagentHistories?: Record<string, SubagentConversationHistory>;
  onLoadSubagentHistory: (agentId: string, force?: boolean) => void;
  messageDurations: Record<string, number>;
  messageFileChanges: Record<string, readonly TurnFileChange[]>;
  undoableUserMessageId?: string;
  onUndoUserMessage: (message: RenderMessage) => void;
  copiedMessageId?: string;
  onCopy: (message: ProtocolMessage) => void;
  onSkillOpen: (name: string) => void;
  onPluginCommandOpen: (command: PluginCommandDetail) => void;
  onCompactionSummaryOpen: (message: RenderMessage) => void;
  compactionEvent?: CompactionEvent;
}) {
  const [processOpen, setProcessOpen] = useState(false);
  const finalResponse = finalResponseMessage(turn.responses);
  const processResponses = finalResponse
    ? turn.responses.filter((message) => message.id !== finalResponse.id)
    : [];
  const hasCollapsedProcess =
    finalResponse !== undefined && processResponses.length > 0;
  const recordedDuration = finalResponse
    ? messageDurations[finalResponse.id]
    : undefined;
  const inferredDuration =
    finalResponse && turn.user
      ? Date.parse(finalResponse.created_at) - Date.parse(turn.user.created_at)
      : undefined;
  const responseDuration =
    recordedDuration ??
    (inferredDuration !== undefined &&
    Number.isFinite(inferredDuration) &&
    inferredDuration >= 0
      ? inferredDuration
      : undefined);
  const fileChanges = finalResponse
    ? messageFileChanges[finalResponse.id]
    : undefined;

  return (
    <section
      className="conversation-turn"
      data-conversation-turn-id={turn.id}
    >
      {turn.user && (
        <UserMessageView
          message={turn.user}
          toolResults={toolResults}
          subagentRuns={subagentRuns}
          subagentHistories={subagentHistories}
          onLoadSubagentHistory={onLoadSubagentHistory}
          onSkillOpen={onSkillOpen}
          onPluginCommandOpen={onPluginCommandOpen}
          canUndo={turn.user.id === undoableUserMessageId}
          onUndo={onUndoUserMessage}
        />
      )}
      {turn.responses.length > 0 && (
        <article className="message assistant-message">
          <div className="assistant-body">
            {hasCollapsedProcess ? (
              <>
                <button
                  type="button"
                  className="turn-process-toggle"
                  aria-expanded={processOpen}
                  onClick={() => setProcessOpen((value) => !value)}
                >
                  <span>
                    {t("history.processed")}
                    {responseDuration !== undefined
                      ? ` ${formatElapsedDuration(responseDuration)}`
                      : ""}
                  </span>
                  {processOpen ? (
                    <ChevronDown size={14} />
                  ) : (
                    <ChevronRight size={14} />
                  )}
                </button>
                <Collapsible
                  open={processOpen}
                  className="turn-process-collapsible"
                >
                  <div className="turn-process-messages">
                    {processResponses.map((message) => (
                      <AssistantMessagePart
                        key={message.id}
                        message={message}
                        toolResults={toolResults}
                        subagentRuns={subagentRuns}
                        subagentHistories={subagentHistories}
                        onLoadSubagentHistory={onLoadSubagentHistory}
                        onCompactionSummaryOpen={onCompactionSummaryOpen}
                        compactionEvent={compactionEvent}
                      />
                    ))}
                  </div>
                </Collapsible>
                <AssistantMessagePart
                  message={finalResponse}
                  toolResults={toolResults}
                  subagentRuns={subagentRuns}
                  subagentHistories={subagentHistories}
                  onLoadSubagentHistory={onLoadSubagentHistory}
                  onCompactionSummaryOpen={onCompactionSummaryOpen}
                  compactionEvent={compactionEvent}
                />
              </>
            ) : (
              turn.responses.map((message) => (
                <AssistantMessagePart
                  key={message.id}
                  message={message}
                  toolResults={toolResults}
                  subagentRuns={subagentRuns}
                  subagentHistories={subagentHistories}
                  onLoadSubagentHistory={onLoadSubagentHistory}
                  onCompactionSummaryOpen={onCompactionSummaryOpen}
                  compactionEvent={compactionEvent}
                />
              ))
            )}
            {fileChanges && fileChanges.length > 0 && (
              <TurnFileChangesCard files={fileChanges} />
            )}
            {finalResponse && (
              <div className="message-actions">
                <button
                  type="button"
                  title={copiedMessageId === finalResponse.id ? t("common.copied") : t("common.copy")}
                  aria-label={copiedMessageId === finalResponse.id ? t("common.copied") : t("common.copy")}
                  onClick={() => onCopy(finalResponse)}
                >
                  {copiedMessageId === finalResponse.id ? (
                    <Check size={14} />
                  ) : (
                    <Copy size={14} />
                  )}
                </button>
              </div>
            )}
            {finalResponse &&
              !hasCollapsedProcess &&
              recordedDuration !== undefined && (
                <AssistantResponseStatus
                  running={false}
                  durationMs={recordedDuration}
                />
              )}
          </div>
        </article>
      )}
    </section>
  );
});

function TurnFileChangesCard({
  files,
}: {
  files: readonly TurnFileChange[];
}) {
  const [open, setOpen] = useState(true);
  const additions = files.reduce(
    (total, file) => total + (file.additions ?? 0),
    0,
  );
  const deletions = files.reduce(
    (total, file) => total + (file.deletions ?? 0),
    0,
  );
  const hasLineStats = files.some(
    (file) => file.additions !== undefined || file.deletions !== undefined,
  );

  return (
    <section className="turn-file-changes">
      <button
        type="button"
        className="turn-file-changes-summary"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="turn-file-changes-icon">
          <FileDiff size={17} />
        </span>
        <span className="turn-file-changes-title">
          {t("filesChanged.title", { count: files.length })}
        </span>
        {hasLineStats && (
          <span className="turn-file-changes-totals">
            <span className="additions">+{additions}</span>
            <span className="deletions">-{deletions}</span>
          </span>
        )}
        {open ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
      </button>
      <Collapsible open={open}>
        <div className="turn-file-changes-list">
          {files.map((file) => (
            <div className="turn-file-change-row" key={file.path}>
              <span className={`file-change-status ${file.change}`}>
                {file.change === "created"
                  ? "+"
                  : file.change === "deleted"
                    ? "-"
                    : "~"}
              </span>
              <span className="turn-file-change-path">{file.path}</span>
              {(file.additions !== undefined ||
                file.deletions !== undefined) && (
                <span className="turn-file-change-lines">
                  <span className="additions">+{file.additions ?? 0}</span>
                  <span className="deletions">-{file.deletions ?? 0}</span>
                </span>
              )}
            </div>
          ))}
        </div>
      </Collapsible>
    </section>
  );
}

function UserMessageView({
  message,
  toolResults,
  subagentRuns,
  subagentHistories,
  onLoadSubagentHistory,
  onSkillOpen,
  onPluginCommandOpen,
  canUndo = false,
  onUndo,
}: {
  message: RenderMessage;
  toolResults: Map<string, ToolResultContent>;
  subagentRuns?: SubagentRunsByTool;
  subagentHistories?: Record<string, SubagentConversationHistory>;
  onLoadSubagentHistory: (agentId: string, force?: boolean) => void;
  onSkillOpen: (name: string) => void;
  onPluginCommandOpen: (command: PluginCommandDetail) => void;
  canUndo?: boolean;
  onUndo?: (message: RenderMessage) => void;
}) {
  const text = messageText(message);
  const pluginCommand = messagePluginCommand(message);
  const structured = messageStructuredContent(message);
  const cronFire = parseCronFireMessage(text);
  return (
    <article className={`message user-message${cronFire ? " cron-fire-message" : ""}`}>
      <div className="message-meta">
        <time>{formatTime(message.created_at)}</time>
      </div>
      <div className={`user-bubble${cronFire ? " cron-fire-bubble" : ""}`}>
        {cronFire ? (
          <CronFireMessageContent fire={cronFire} />
        ) : pluginCommand ? (
          <PluginCommandDisplayContent
            command={pluginCommand}
            onOpen={() =>
              onPluginCommandOpen({
                ...pluginCommand,
                id: message.id,
                content: text,
                createdAt: message.created_at,
              })
            }
          />
        ) : (
          <SkillPromptDisplayContent
            text={text}
            onSkillOpen={onSkillOpen}
          />
        )}
        <StructuredMessageContent
          parts={structured}
          toolResults={toolResults}
          subagentRuns={subagentRuns}
          subagentHistories={subagentHistories}
          onLoadSubagentHistory={onLoadSubagentHistory}
        />
      </div>
      {canUndo && onUndo && (
        <div className="user-message-actions">
          <button
            type="button"
            className="user-message-undo"
            title={t("undo.tooltip")}
            aria-label={t("undo.tooltip")}
            onClick={() => onUndo(message)}
          >
            <Undo2 size={14} />
          </button>
        </div>
      )}
    </article>
  );
}

function AssistantMessagePart({
  message,
  toolResults,
  subagentRuns,
  subagentHistories,
  onLoadSubagentHistory,
  onCompactionSummaryOpen,
  compactionEvent,
}: {
  message: RenderMessage;
  toolResults: Map<string, ToolResultContent>;
  subagentRuns?: SubagentRunsByTool;
  subagentHistories?: Record<string, SubagentConversationHistory>;
  onLoadSubagentHistory?: (agentId: string, force?: boolean) => void;
  onCompactionSummaryOpen: (message: RenderMessage) => void;
  compactionEvent?: CompactionEvent;
}) {
  const text = messageText(message);
  const thinking = messageThinking(message);
  const structured = messageStructuredContent(message);

  if (messageOriginKind(message) === "compaction_summary") {
    const tokenTransition = compactionTokenTransition(compactionEvent);
    return (
      <div className="history-summary-divider" role="separator">
        <span aria-hidden="true" />
        <strong>
          {t("compaction.completed")}
          {tokenTransition ? t("compaction.tokens", { transition: tokenTransition }) : ""}
        </strong>
        <button
          type="button"
          onClick={() => onCompactionSummaryOpen(message)}
        >
          {t("compaction.viewSummary")}
        </button>
        <span aria-hidden="true" />
      </div>
    );
  }

  if (!thinking && !text && structured.length === 0) return null;

  return (
    <div className={`assistant-message-part ${message.status ?? ""}`}>
      {thinking && <ThinkingSummary content={thinking} />}
      {(text || structured.length > 0) && (
        <div className="markdown-body">
          {text && <MarkdownMessage content={text} />}
          <StructuredMessageContent
            parts={structured}
            toolResults={toolResults}
            subagentRuns={subagentRuns}
            subagentHistories={subagentHistories}
            onLoadSubagentHistory={onLoadSubagentHistory}
          />
        </div>
      )}
    </div>
  );
}


function MessageImage({
  src,
  alt,
  path,
}: {
  src: string;
  alt: string;
  path?: string;
}) {
  return (
    <PreviewableImage
      className="history-media"
      src={src}
      alt={alt}
      path={path}
    />
  );
}

function MessageAudio({ src }: { src: string }) {
  return (
    <audio className="history-media" src={src} controls preload="metadata" />
  );
}

function MessageVideo({ src }: { src: string }) {
  return (
    <video className="history-media" src={src} controls preload="metadata" />
  );
}

export function PromptAttachmentContent({
  attachments,
}: {
  attachments: readonly PromptAttachment[];
}) {
  if (attachments.length === 0) return null;
  return (
    <div className="structured-content">
      {attachments.map((attachment) => {
        switch (attachment.kind) {
          case "image":
            return (
              <MessageImage
                src={attachment.dataUrl!}
                alt={attachment.name}
                path={attachment.name}
                key={attachment.id}
              />
            );
          case "audio":
            return (
              <MessageAudio src={attachment.dataUrl!} key={attachment.id} />
            );
          case "video":
            return (
              <MessageVideo src={attachment.dataUrl!} key={attachment.id} />
            );
          case "file":
            return (
              <div className="history-file" key={attachment.id}>
                <FileCode2 size={13} />
                <span>{attachment.name}</span>
                <small>{formatBytes(attachment.size)}</small>
              </div>
            );
        }
      })}
    </div>
  );
}

function editToolInput(
  input: unknown,
): { path?: string; oldString?: string; newString?: string } | undefined {
  if (!input || typeof input !== "object") return undefined;
  const record = input as Record<string, unknown>;
  const oldString = typeof record.old_string === "string" ? record.old_string : undefined;
  const newString = typeof record.new_string === "string" ? record.new_string : undefined;
  if (oldString === undefined && newString === undefined) return undefined;
  return {
    path: typeof record.path === "string" ? record.path : undefined,
    oldString,
    newString,
  };
}

function EditDiffLine({
  kind,
  lineno,
  text,
}: {
  kind: "removed" | "added";
  lineno: number;
  text: string;
}) {
  return (
    <div className={`edit-diff-line ${kind}`}>
      <span className="edit-diff-lineno">{lineno}</span>
      <span className="edit-diff-sign">{kind === "removed" ? "-" : "+"}</span>
      <span className="edit-diff-code">{text}</span>
    </div>
  );
}

function EditToolDiff({ input }: { input: unknown }) {
  const edit = editToolInput(input);
  if (!edit) return null;
  const removed = edit.oldString?.replace(/\r?\n$/, "").split(/\r?\n/);
  const added =
    edit.newString === ""
      ? undefined
      : edit.newString?.replace(/\r?\n$/, "").split(/\r?\n/);
  return (
    <div className="edit-diff">
      {edit.path && (
        <div className="edit-diff-header">
          <FileCode2 size={12} />
          <span>{edit.path}</span>
        </div>
      )}
      <div className="edit-diff-body">
        {removed?.map((line, index) => (
          <EditDiffLine
            kind="removed"
            lineno={index + 1}
            text={line}
            key={`removed-${index}`}
          />
        ))}
        {added?.map((line, index) => (
          <EditDiffLine
            kind="added"
            lineno={index + 1}
            text={line}
            key={`added-${index}`}
          />
        ))}
      </div>
    </div>
  );
}

function writeToolInput(
  input: unknown,
): { path?: string; content: string } | undefined {
  if (!input || typeof input !== "object") return undefined;
  const record = input as Record<string, unknown>;
  if (typeof record.content !== "string") return undefined;
  return {
    path: typeof record.path === "string" ? record.path : undefined,
    content: record.content,
  };
}

function WriteToolContent({ input }: { input: unknown }) {
  const write = writeToolInput(input);
  if (!write) return null;
  const lines = write.content.replace(/\r?\n$/, "").split(/\r?\n/);
  return (
    <div className="edit-diff">
      {write.path && (
        <div className="edit-diff-header">
          <FileCode2 size={12} />
          <span>{write.path}</span>
        </div>
      )}
      <div className="edit-diff-body">
        {lines.map((line, index) => (
          <div className="edit-diff-line context" key={index}>
            <span className="edit-diff-lineno">{index + 1}</span>
            <span className="edit-diff-code">{line}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function agentToolInput(
  input: unknown,
): { description: string; prompt: string } | undefined {
  if (!input || typeof input !== "object") return undefined;
  const record = input as Record<string, unknown>;
  if (typeof record.prompt !== "string") return undefined;
  return {
    description:
      typeof record.description === "string" ? record.description : "",
    prompt: record.prompt,
  };
}

function AgentToolCopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className="agent-tool-copy"
      title={copied ? t("common.copied") : t("common.copy")}
      aria-label={copied ? t("common.copied") : t("common.copy")}
      onClick={() => {
        void navigator.clipboard.writeText(text).then(() => {
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1400);
        });
      }}
    >
      {copied ? <Check size={12} /> : <Copy size={12} />}
    </button>
  );
}

function AgentToolContent({ input }: { input: unknown }) {
  const agent = agentToolInput(input);
  if (!agent) return null;
  return (
    <div className="agent-tool">
      <div className="agent-tool-header">
        <Bot size={12} />
        <span className="agent-tool-title">{agent.description || "Agent"}</span>
        <AgentToolCopyButton text={agent.prompt} />
      </div>
      <div className="agent-tool-body markdown-body">
        <MarkdownMessage content={agent.prompt} />
      </div>
    </div>
  );
}

type AgentSwarmToolInputView = {
  description: string;
  subagentType?: string;
  promptTemplate?: string;
  items: string[];
  resumeAgents: Array<{ agentId: string; prompt: string }>;
};

function agentSwarmToolInput(
  input: unknown,
): AgentSwarmToolInputView | undefined {
  if (!input || typeof input !== "object") return undefined;
  const record = input as Record<string, unknown>;
  if (typeof record.description !== "string") return undefined;

  const items = Array.isArray(record.items)
    ? record.items.filter((item): item is string => typeof item === "string")
    : [];
  const resumeAgents =
    record.resume_agent_ids &&
    typeof record.resume_agent_ids === "object" &&
    !Array.isArray(record.resume_agent_ids)
      ? Object.entries(record.resume_agent_ids)
          .filter((entry): entry is [string, string] =>
            typeof entry[1] === "string"
          )
          .map(([agentId, prompt]) => ({ agentId, prompt }))
      : [];

  return {
    description: record.description,
    subagentType:
      typeof record.subagent_type === "string"
        ? record.subagent_type
        : undefined,
    promptTemplate:
      typeof record.prompt_template === "string"
        ? record.prompt_template
        : undefined,
    items,
    resumeAgents,
  };
}

function AgentSwarmToolContent({ input }: { input: unknown }) {
  const swarm = agentSwarmToolInput(input);
  if (!swarm) return null;

  return (
    <div className="agent-swarm-tool">
      <header className="agent-swarm-header">
        <Bot size={13} />
        <div className="agent-swarm-heading">
          <strong>{swarm.description || "AgentSwarm"}</strong>
          <div className="agent-swarm-meta">
            {swarm.subagentType && (
              <span>
                {t("agentSwarm.subagentType", { type: swarm.subagentType })}
              </span>
            )}
            {swarm.items.length > 0 && (
              <span>{t("agentSwarm.itemCount", { count: swarm.items.length })}</span>
            )}
            {swarm.resumeAgents.length > 0 && (
              <span>
                {t("agentSwarm.resumeCount", {
                  count: swarm.resumeAgents.length,
                })}
              </span>
            )}
          </div>
        </div>
      </header>

      {swarm.promptTemplate && (
        <section className="agent-swarm-section agent-swarm-template">
          <header>
            <span>{t("agentSwarm.promptTemplate")}</span>
            <AgentToolCopyButton text={swarm.promptTemplate} />
          </header>
          <div className="agent-swarm-template-body markdown-body">
            <MarkdownMessage content={swarm.promptTemplate} />
          </div>
        </section>
      )}

      {swarm.items.length > 0 && (
        <section className="agent-swarm-section">
          <header>
            <span>{t("agentSwarm.items")}</span>
            <small>{swarm.items.length}</small>
          </header>
          <ol className="agent-swarm-items">
            {swarm.items.map((item, index) => (
              <li key={`${index}-${item}`}>
                <span>{item}</span>
              </li>
            ))}
          </ol>
        </section>
      )}

      {swarm.resumeAgents.length > 0 && (
        <section className="agent-swarm-section">
          <header>
            <span>{t("agentSwarm.resumeAgents")}</span>
            <small>{swarm.resumeAgents.length}</small>
          </header>
          <div className="agent-swarm-resume-list">
            {swarm.resumeAgents.map((agent) => (
              <article key={agent.agentId}>
                <strong>{agent.agentId}</strong>
                <p>{agent.prompt}</p>
              </article>
            ))}
          </div>
        </section>
      )}
    </div>
  );
}

const AGENT_RESULT_SUMMARY_MARKER = "[summary]";

function agentToolResultText(output: unknown): string | undefined {
  if (typeof output !== "string") return undefined;
  const markerIndex = output.indexOf(AGENT_RESULT_SUMMARY_MARKER);
  const text =
    markerIndex >= 0
      ? output.slice(markerIndex + AGENT_RESULT_SUMMARY_MARKER.length)
      : output;
  return text.replace(/^\r?\n/, "");
}

function AgentToolResult({
  text,
  isError,
}: {
  text: string;
  isError?: boolean;
}) {
  return (
    <div className="agent-tool">
      <div className="agent-tool-header">
        <Bot size={12} />
        <span className="agent-tool-title">{t("tool.result")}</span>
        <AgentToolCopyButton text={text} />
      </div>
      <div className={`agent-tool-body markdown-body${isError ? " error" : ""}`}>
        <MarkdownMessage content={text} />
      </div>
    </div>
  );
}

export function ToolInputView({
  name,
  input,
  streamingInput,
}: {
  name: string | undefined;
  input: unknown;
  streamingInput?: Record<string, string>;
}) {
  if (name === "Edit") {
    const editInput = editToolInput(input) ? input : streamingInput;
    if (editToolInput(editInput)) return <EditToolDiff input={editInput} />;
  }
  if (name === "Write") {
    const writeInput = writeToolInput(input) ? input : streamingInput;
    if (writeToolInput(writeInput)) return <WriteToolContent input={writeInput} />;
  }
  if (name === "Agent" && agentToolInput(input)) {
    return <AgentToolContent input={input} />;
  }
  if (name === "AgentSwarm" && agentSwarmToolInput(input)) {
    return <AgentSwarmToolContent input={input} />;
  }
  return <pre>{structuredValue(input)}</pre>;
}

export function ToolResultView({
  name,
  output,
  isError,
}: {
  name: string | undefined;
  output: unknown;
  isError?: boolean;
}) {
  if (name === "AgentSwarm" && !isError) return null;
  const agentText = name === "Agent" ? agentToolResultText(output) : undefined;
  if (agentText !== undefined) {
    return (
      <section className="tool-detail-section">
        <AgentToolResult text={agentText} isError={isError} />
      </section>
    );
  }
  if (name === "ExitPlanMode" && !isError && typeof output === "string") {
    return (
      <section className="tool-detail-section">
        <div className="exit-plan-mode-result-heading">
          <span>{t("tool.result")}</span>
          <AgentToolCopyButton text={output} />
        </div>
        <div className="exit-plan-mode-result markdown-body">
          <MarkdownMessage content={output} />
        </div>
      </section>
    );
  }
  return (
    <section className="tool-detail-section">
      <span>{t("tool.result")}</span>
      <pre className={isError ? "error" : ""}>{structuredValue(output)}</pre>
    </section>
  );
}

function HistoryToolCard({
  tool,
  result,
  subagents,
  subagentRuns,
  subagentHistories,
  onLoadSubagentHistory,
}: {
  tool: Extract<MessageContent, { type: "tool_use" }>;
  result?: ToolResultContent;
  subagents: readonly SubagentRun[];
  subagentRuns?: SubagentRunsByTool;
  subagentHistories?: Record<string, SubagentConversationHistory>;
  onLoadSubagentHistory?: (agentId: string, force?: boolean) => void;
}) {
  const [open, setOpen] = useState(false);
  const status = result
    ? result.is_error
      ? "error"
      : "completed"
    : "incomplete";
  const summary = toolInputSummary(tool.input);
  const displayedSubagents = subagentRunsWithSwarmItems(subagents, tool.input);

  return (
    <div className={`history-tool-card ${status}`}>
      <button
        type="button"
        className="tool-card-summary"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <ToolStatusIcon status={status} />
        <Wrench size={13} />
        <span className="tool-card-name">{tool.tool_name}</span>
        {summary && (
          <span className="tool-card-preview" title={summary}>
            {summary}
          </span>
        )}
        <small>
          {result ? (result.is_error ? t("status.failed") : t("status.completed")) : t("status.incomplete")}
        </small>
      </button>
      {displayedSubagents.length > 0 && (
        <SubagentPanel
          subagents={displayedSubagents}
          nestedRuns={subagentRuns}
          histories={subagentHistories}
          onLoadHistory={onLoadSubagentHistory}
          parentActive={false}
        />
      )}
      <Collapsible className="tool-card-collapse" open={open}>
        <div className="history-tool-detail">
          <section className="tool-detail-section">
            <span>{t("tool.params")}</span>
            <ToolInputView name={tool.tool_name} input={tool.input} />
          </section>
          {result && (
            <ToolResultView
              name={tool.tool_name}
              output={result.output}
              isError={result.is_error}
            />
          )}
        </div>
      </Collapsible>
    </div>
  );
}

export function StructuredMessageContent({
  parts,
  toolResults,
  subagentRuns,
  subagentHistories,
  onLoadSubagentHistory,
}: {
  parts: MessageContent[];
  toolResults: Map<string, ToolResultContent>;
  subagentRuns?: SubagentRunsByTool;
  subagentHistories?: Record<string, SubagentConversationHistory>;
  onLoadSubagentHistory?: (agentId: string, force?: boolean) => void;
}) {
  if (parts.length === 0) return null;
  return (
    <div className="structured-content">
      {parts.map((part, index) => {
        switch (part.type) {
          case "tool_use": {
            const result = toolResults.get(part.tool_call_id);
            return (
              <HistoryToolCard
                tool={part}
                result={result}
                subagents={subagentRuns?.[part.tool_call_id] ?? []}
                subagentRuns={subagentRuns}
                subagentHistories={subagentHistories}
                onLoadSubagentHistory={onLoadSubagentHistory}
                key={`${part.tool_call_id}-${index}`}
              />
            );
          }
          case "tool_result":
            return null;
          case "image": {
            const url = mediaSourceUrl(part.source);
            return url ? (
              <MessageImage src={url} alt={t("message.sessionImageAlt")} key={index} />
            ) : (
              <div className="history-file" key={index}>
                {t("message.imageFile", { id: part.source.kind === "file" ? part.source.file_id : "" })}
              </div>
            );
          }
          case "audio": {
            const url = mediaSourceUrl(part.source);
            return url ? (
              <MessageAudio src={url} key={index} />
            ) : (
              <div className="history-file" key={index}>
                {t("message.audioFile", { id: part.source.kind === "file" ? part.source.file_id : "" })}
              </div>
            );
          }
          case "video": {
            const url = mediaSourceUrl(part.source);
            return url ? (
              <MessageVideo src={url} key={index} />
            ) : (
              <div className="history-file" key={index}>
                {t("message.videoFile", { id: part.source.kind === "file" ? part.source.file_id : "" })}
              </div>
            );
          }
          case "file":
            return (
              <div className="history-file" key={`${part.file_id}-${index}`}>
                <FileCode2 size={13} />
                <span>{part.name || part.file_id}</span>
                <small>{part.media_type}</small>
              </div>
            );
          case "text":
          case "thinking":
            return null;
        }
      })}
    </div>
  );
}
