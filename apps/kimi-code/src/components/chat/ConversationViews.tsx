import {
  Fragment,
  type ReactNode,
  memo,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import {
  ArrowUp,
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  Code2,
  Copy,
  FileCode2,
  FileDiff,
  MoreHorizontal,
  MessageSquareText,
  Square,
  TerminalSquare,
  Undo2,
  Wrench,
  X,
} from "lucide-react";

import {
  finalResponseMessage,
  formatElapsedDuration,
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
import {
  mediaSourceUrl,
  messageStructuredContent,
  messageText,
  messageThinking,
  structuredValue,
} from "../../chat/messages";
import { t } from "../../i18n";
import { SkillPromptDisplayContent } from "../../prompt/SkillPromptDisplay";
import {
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
}: {
  turn: InFlightTurn;
  outlineId?: string;
  subagentRuns?: SubagentRunsByTool;
  subagentLiveTurns?: Record<string, InFlightTurn>;
  onSkillOpen: (name: string) => void;
}) {
  const hasBlocks = turn.steps.some((step) => step.blocks.length > 0);
  const streaming = isTurnRunning(turn);

  return (
    <section
      className="conversation-turn live-conversation-turn"
      data-conversation-turn-id={outlineId}
    >
      <article className="message user-message live-user-message">
        <div className="message-meta">
          <time>{formatTime(turn.createdAt)}</time>
        </div>
        <div className="user-bubble">
          <SkillPromptDisplayContent
            text={turn.prompt}
            skills={turn.skills}
            onSkillOpen={onSkillOpen}
          />
          <PromptAttachmentContent attachments={turn.attachments} />
        </div>
      </article>
      <article className={`message assistant-message live-turn ${turn.status}`}>
        <div className="assistant-body">
          {turn.steps.map((step) => {
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
          />
        </div>
      </article>
    </section>
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
          skills={message.skills.map((skill) => skill.name)}
          onSkillOpen={onSkillOpen}
        />
        <PromptAttachmentContent attachments={message.attachments} />
      </div>
    </div>
  );
}

export function QueuedPromptList({
  prompts,
  canSteer,
  onRemove,
  onSteer,
  onSkillOpen,
}: {
  prompts: readonly QueuedPrompt[];
  canSteer: boolean;
  onRemove: (queuedPromptId: string) => void;
  onSteer: (queuedPromptId: string) => void;
  onSkillOpen: (name: string) => void;
}) {
  return (
    <section className="queued-prompt-stack" aria-label={t("queue.ariaLabel")}>
      <header>
        <span>
          <MessageSquareText size={13} />
          {t("queue.title", { count: prompts.length })}
        </span>
        <small>{t("queue.hint")}</small>
      </header>
      {prompts.map((prompt, index) => (
        <article className="queued-prompt" key={prompt.id}>
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
                {t("queue.attachmentsOnly", { count: prompt.attachments.length })}
              </span>
            )}
            <PromptAttachmentContent attachments={prompt.attachments} />
          </div>
          <footer>
            <span className={index === 0 ? "next" : ""}>
              {index === 0 ? t("queue.next") : `#${index + 1}`}
            </span>
            <div>
              <button
                type="button"
                disabled={
                  !canSteer ||
                  prompt.steering ||
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
                {prompt.steering ? <span className="spinner" /> : <ArrowUp size={13} />}
                {t("queue.steer")}
              </button>
              <button
                type="button"
                disabled={prompt.steering}
                title={t("queue.withdraw")}
                aria-label={t("queue.withdrawAria")}
                onClick={() => onRemove(prompt.id)}
              >
                <X size={13} />
                {t("queue.withdraw")}
              </button>
            </div>
          </footer>
        </article>
      ))}
    </section>
  );
}

export function RemoteQueuedPromptList({
  prompts,
  onSkillOpen,
}: {
  prompts: readonly RemoteQueuedPrompt[];
  onSkillOpen: (name: string) => void;
}) {
  return (
    <section className="queued-prompt-stack" aria-label={t("queue.remoteAriaLabel")}>
      <header>
        <span>
          <MessageSquareText size={13} />
          {t("queue.remoteTitle", { count: prompts.length })}
        </span>
        <small>{t("queue.remoteHint")}</small>
      </header>
      {prompts.map((prompt, index) => (
        <article className="queued-prompt" key={prompt.promptId}>
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
                {t("queue.attachmentsOnly", { count: prompt.attachments.length })}
              </span>
            )}
            <PromptAttachmentContent attachments={prompt.attachments} />
          </div>
          <footer>
            <span className={index === 0 ? "next" : ""}>
              {index === 0 ? t("queue.next") : `#${index + 1}`}
            </span>
            <small>{formatTime(prompt.createdAt)}</small>
          </footer>
        </article>
      ))}
    </section>
  );
}

export function AssistantResponseStatus({
  running,
  durationMs,
}: {
  running: boolean;
  durationMs?: number;
}) {
  if (!running && durationMs === undefined) return null;
  return (
    <div
      className={`assistant-response-status ${running ? "thinking" : "elapsed"}`}
      aria-live="polite"
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
        <img
          className="history-media"
          src={content.imageUrl.url}
          alt={t("message.imageAlt")}
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
  const [open, setOpen] = useState(active);
  const userToggled = useRef(false);
  useEffect(() => {
    if (!userToggled.current) setOpen(active);
  }, [active]);
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
  const displayedSubagents = subagentRunsWithSwarmItems(subagents, input);
  return (
    <div className={`live-tool-card ${tool.status}`}>
      <button
        type="button"
        className="tool-card-summary"
        aria-expanded={open}
        onClick={() => {
          userToggled.current = true;
          setOpen((value) => !value);
        }}
      >
        <ToolStatusIcon status={tool.status} />
        <Wrench size={13} />
        <span>{tool.name ?? t("tool.preparing")}</span>
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
              <ToolInputView name={tool.name} input={input} />
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
  parentActive,
}: {
  subagents: readonly SubagentRun[];
  liveTurns?: Record<string, InFlightTurn>;
  nestedRuns?: SubagentRunsByTool;
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
  const [open, setOpen] = useState(active);
  const userToggled = useRef(false);
  const listScroll = useRef<HTMLDivElement>(null);
  const followLatestList = useRef(true);

  useEffect(() => {
    if (!userToggled.current) setOpen(active);
  }, [active]);
  useLayoutEffect(() => {
    if (!open || !followLatestList.current || !listScroll.current) return;
    listScroll.current.scrollTop = listScroll.current.scrollHeight;
  }, [liveTurns, subagents]);

  return (
    <section
      className={`subagent-panel ${active ? "active" : "settled"}`}
      aria-label={t("subagent.progressAria")}
    >
      <button
        type="button"
        className="subagent-panel-summary"
        aria-expanded={open}
        onClick={() => {
          userToggled.current = true;
          setOpen((value) => !value);
        }}
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
          ref={listScroll}
          onScroll={(event) => {
            const target = event.currentTarget;
            followLatestList.current =
              target.scrollHeight - target.scrollTop - target.clientHeight <=
              24;
          }}
        >
          {subagents.map((subagent, index) => (
            <SubagentRow
              key={subagent.subagentId}
              subagent={subagent}
              status={statuses[index]}
              liveTurn={liveTurns?.[subagent.subagentId]}
              liveTurns={liveTurns}
              nestedRuns={nestedRuns}
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
}: {
  subagent: SubagentRun;
  status: DisplaySubagentStatus;
  liveTurn?: InFlightTurn;
  liveTurns?: Record<string, InFlightTurn>;
  nestedRuns?: SubagentRunsByTool;
}) {
  const hasDetail =
    liveTurn !== undefined ||
    Boolean(subagent.resultSummary) ||
    Boolean(subagent.error) ||
    subagent.usage !== undefined ||
    subagent.contextTokens !== undefined;
  const active =
    status === "queued" ||
    status === "running" ||
    status === "suspended";
  const [open, setOpen] = useState(active);
  const userToggled = useRef(false);
  useEffect(() => {
    if (!userToggled.current) setOpen(active);
  }, [active]);
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
          userToggled.current = true;
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
          {subagent.resultSummary && (
            <section className="subagent-result-summary">
              <span>{t("subagent.finalSummary")}</span>
              <pre>{subagent.resultSummary}</pre>
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
  subagentLiveTurns,
  messageDurations,
  messageFileChanges,
  undoableUserMessageId,
  onUndoUserMessage,
  copiedMessageId,
  onCopy,
  onSkillOpen,
  onCompactionSummaryOpen,
  compactionEvent,
}: {
  turn: HistoryConversationTurn;
  toolResults: Map<string, ToolResultContent>;
  subagentRuns?: SubagentRunsByTool;
  subagentLiveTurns?: Record<string, InFlightTurn>;
  messageDurations: Record<string, number>;
  messageFileChanges: Record<string, readonly TurnFileChange[]>;
  undoableUserMessageId?: string;
  onUndoUserMessage: (message: RenderMessage) => void;
  copiedMessageId?: string;
  onCopy: (message: ProtocolMessage) => void;
  onSkillOpen: (name: string) => void;
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
          subagentLiveTurns={subagentLiveTurns}
          onSkillOpen={onSkillOpen}
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
                        subagentLiveTurns={subagentLiveTurns}
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
                  subagentLiveTurns={subagentLiveTurns}
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
                  subagentLiveTurns={subagentLiveTurns}
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
                <button onClick={() => onCopy(finalResponse)}>
                  {copiedMessageId === finalResponse.id ? (
                    <Check size={14} />
                  ) : (
                    <Copy size={14} />
                  )}
                  {copiedMessageId === finalResponse.id ? t("common.copied") : t("common.copy")}
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
  subagentLiveTurns,
  onSkillOpen,
  canUndo = false,
  onUndo,
}: {
  message: RenderMessage;
  toolResults: Map<string, ToolResultContent>;
  subagentRuns?: SubagentRunsByTool;
  subagentLiveTurns?: Record<string, InFlightTurn>;
  onSkillOpen: (name: string) => void;
  canUndo?: boolean;
  onUndo?: (message: RenderMessage) => void;
}) {
  const text = messageText(message);
  const structured = messageStructuredContent(message);
  return (
    <article className="message user-message">
      <div className="message-meta">
        <time>{formatTime(message.created_at)}</time>
      </div>
      <div className="user-bubble">
        <SkillPromptDisplayContent
          text={text}
          onSkillOpen={onSkillOpen}
        />
        <StructuredMessageContent
          parts={structured}
          toolResults={toolResults}
          subagentRuns={subagentRuns}
          subagentLiveTurns={subagentLiveTurns}
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
  subagentLiveTurns,
  onCompactionSummaryOpen,
  compactionEvent,
}: {
  message: RenderMessage;
  toolResults: Map<string, ToolResultContent>;
  subagentRuns?: SubagentRunsByTool;
  subagentLiveTurns?: Record<string, InFlightTurn>;
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
            subagentLiveTurns={subagentLiveTurns}
          />
        </div>
      )}
    </div>
  );
}


function MessageImage({
  src,
  alt,
}: {
  src: string;
  alt: string;
}) {
  return <img className="history-media" src={src} alt={alt} />;
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
): { path?: string; oldString: string; newString: string } | undefined {
  if (!input || typeof input !== "object") return undefined;
  const record = input as Record<string, unknown>;
  if (
    typeof record.old_string !== "string" ||
    typeof record.new_string !== "string"
  ) {
    return undefined;
  }
  return {
    path: typeof record.path === "string" ? record.path : undefined,
    oldString: record.old_string,
    newString: record.new_string,
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
  const removed = edit.oldString.replace(/\r?\n$/, "").split(/\r?\n/);
  const added = edit.newString.replace(/\r?\n$/, "").split(/\r?\n/);
  return (
    <div className="edit-diff">
      {edit.path && (
        <div className="edit-diff-header">
          <FileCode2 size={12} />
          <span>{edit.path}</span>
        </div>
      )}
      <div className="edit-diff-body">
        {removed.map((line, index) => (
          <EditDiffLine
            kind="removed"
            lineno={index + 1}
            text={line}
            key={`removed-${index}`}
          />
        ))}
        {added.map((line, index) => (
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
      onClick={() => {
        void navigator.clipboard.writeText(text).then(() => {
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1400);
        });
      }}
    >
      {copied ? <Check size={12} /> : <Copy size={12} />}
      {copied ? t("common.copied") : t("common.copy")}
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
}: {
  name: string | undefined;
  input: unknown;
}) {
  if (name === "Edit" && editToolInput(input)) {
    return <EditToolDiff input={input} />;
  }
  if (name === "Write" && writeToolInput(input)) {
    return <WriteToolContent input={input} />;
  }
  if (name === "Agent" && agentToolInput(input)) {
    return <AgentToolContent input={input} />;
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
  const agentText = name === "Agent" ? agentToolResultText(output) : undefined;
  if (agentText !== undefined) {
    return (
      <section className="tool-detail-section">
        <AgentToolResult text={agentText} isError={isError} />
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
  subagentLiveTurns,
}: {
  tool: Extract<MessageContent, { type: "tool_use" }>;
  result?: ToolResultContent;
  subagents: readonly SubagentRun[];
  subagentRuns?: SubagentRunsByTool;
  subagentLiveTurns?: Record<string, InFlightTurn>;
}) {
  const [open, setOpen] = useState(false);
  const status = result
    ? result.is_error
      ? "error"
      : "completed"
    : "incomplete";
  const displayedSubagents = subagentRunsWithSwarmItems(
    subagents,
    tool.input,
  );

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
        <span>{tool.tool_name}</span>
        <small>
          {result ? (result.is_error ? t("status.failed") : t("status.completed")) : t("status.incomplete")}
        </small>
      </button>
      {displayedSubagents.length > 0 && (
        <SubagentPanel
          subagents={displayedSubagents}
          liveTurns={subagentLiveTurns}
          nestedRuns={subagentRuns}
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
  subagentLiveTurns,
}: {
  parts: MessageContent[];
  toolResults: Map<string, ToolResultContent>;
  subagentRuns?: SubagentRunsByTool;
  subagentLiveTurns?: Record<string, InFlightTurn>;
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
                subagentLiveTurns={subagentLiveTurns}
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
