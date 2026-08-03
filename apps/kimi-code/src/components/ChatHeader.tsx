import {
  type ReactNode,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  Minus,
  MoreHorizontal,
  Square,
  SquarePen,
  TerminalSquare,
  X,
} from "lucide-react";

import { t } from "../i18n";
import { isDesktop } from "../transport";
import type {
  AgentTaskInfo,
  AgentUsageStatus,
  BackgroundTaskView,
  CompactionEvent,
  ContextUsage,
  Model,
  TodoItem,
  TokenUsage,
} from "../types";
import {
  formatCacheHitRate,
  formatCompactTokenCount,
  formatTokenCount,
  inputTokenUsage,
} from "../utils/format";

function TokenUsageBreakdown({
  label,
  usage,
}: {
  label: string;
  usage?: TokenUsage;
}) {
  return (
    <div className="token-usage-breakdown">
      <strong>{label}</strong>
      <div>
        <span>
          <small>{t("usage.totalInput")}</small>
          <b>{usage ? formatTokenCount(inputTokenUsage(usage)) : "—"}</b>
        </span>
        <span>
          <small>{t("usage.output")}</small>
          <b>{usage ? formatTokenCount(usage.output) : "—"}</b>
        </span>
      </div>
      <div>
        <span>
          <small>{t("usage.cacheInput")}</small>
          <b>{usage ? formatTokenCount(usage.inputCacheRead) : "—"}</b>
        </span>
        <span>
          <small>{t("usage.cacheHitRate")}</small>
          <b>{formatCacheHitRate(usage)}</b>
        </span>
      </div>
    </div>
  );
}

export interface ToolbarSelectOption {
  value: string;
  label: string;
  description?: string;
  danger?: boolean;
}

export function ChatHeaderTitle({
  title,
  onRename,
}: {
  title: string;
  onRename: (nextTitle: string) => void;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(title);

  useEffect(() => {
    if (!menuOpen) return;
    const close = (event: PointerEvent): void => {
      if (!rootRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, [menuOpen]);

  useEffect(() => {
    if (!editing) return;
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [editing]);

  const startEditing = (): void => {
    setDraft(title);
    setMenuOpen(false);
    setEditing(true);
  };

  const commitRename = (): void => {
    const nextTitle = draft.trim();
    setEditing(false);
    if (nextTitle && nextTitle !== title) onRename(nextTitle);
  };

  if (editing) {
    return (
      <input
        ref={inputRef}
        className="chat-title-input"
        value={draft}
        placeholder={t("conversation.renamePlaceholder")}
        aria-label={t("conversation.rename")}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commitRename}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            commitRename();
          } else if (event.key === "Escape") {
            event.preventDefault();
            setEditing(false);
          }
        }}
      />
    );
  }

  return (
    <div className="chat-title" ref={rootRef}>
      <h1 title={title}>{title}</h1>
      <button
        className="icon-button chat-title-more"
        type="button"
        aria-label={t("conversation.rename")}
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        onClick={() => setMenuOpen((current) => !current)}
      >
        <MoreHorizontal size={16} />
      </button>
      {menuOpen && (
        <div className="chat-title-menu" role="menu" aria-label={title}>
          <button type="button" role="menuitem" onClick={startEditing}>
            <SquarePen size={13} />
            {t("conversation.rename")}
          </button>
        </div>
      )}
    </div>
  );
}

export function WindowTitleBar() {
  const [maximized, setMaximized] = useState(false);
  const appWindow = useMemo(
    () => (isDesktop() ? getCurrentWindow() : undefined),
    [],
  );

  useEffect(() => {
    if (!appWindow) return;

    let disposed = false;
    let unlisten: (() => void) | undefined;
    const syncMaximized = (): void => {
      void appWindow
        .isMaximized()
        .then((value) => {
          if (!disposed) setMaximized(value);
        })
        .catch(() => undefined);
    };

    syncMaximized();
    void appWindow
      .onResized(syncMaximized)
      .then((dispose) => {
        if (disposed) dispose();
        else unlisten = dispose;
      })
      .catch(() => undefined);

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [appWindow]);

  const runWindowAction = (action: "minimize" | "close"): void => {
    if (!appWindow) return;
    void appWindow[action]().catch(() => undefined);
  };

  const toggleMaximize = (): void => {
    if (!appWindow) return;
    void appWindow
      .toggleMaximize()
      .then(() => appWindow.isMaximized())
      .then(setMaximized)
      .catch(() => undefined);
  };

  return (
    <header className="window-titlebar" data-tauri-drag-region>
      <div
        className="window-titlebar-brand"
        data-tauri-drag-region
        aria-label="Kimi Code"
      >
        <div className="brand-mark compact" data-tauri-drag-region>
          <span data-tauri-drag-region />
          <span data-tauri-drag-region />
        </div>
        <div className="titlebar-brand-copy" data-tauri-drag-region>
          <strong data-tauri-drag-region>Kimi Code</strong>
          <span data-tauri-drag-region>
            {isDesktop() ? "Agent Desktop" : "Agent Web"}
          </span>
        </div>
      </div>

      {appWindow && <div className="window-controls">
        <button
          className="window-control"
          type="button"
          title={t("window.minimize")}
          aria-label={t("window.minimizeWindow")}
          onClick={() => runWindowAction("minimize")}
        >
          <Minus size={15} strokeWidth={1.7} />
        </button>
        <button
          className="window-control"
          type="button"
          title={maximized ? t("window.restore") : t("window.maximize")}
          aria-label={maximized ? t("window.restoreWindow") : t("window.maximizeWindow")}
          onClick={toggleMaximize}
        >
          {maximized ? (
            <Copy className="restore-icon" size={12} strokeWidth={1.5} />
          ) : (
            <Square size={11} strokeWidth={1.5} />
          )}
        </button>
        <button
          className="window-control close"
          type="button"
          title={t("window.close")}
          aria-label={t("window.closeWindow")}
          onClick={() => runWindowAction("close")}
        >
          <X size={15} strokeWidth={1.7} />
        </button>
      </div>}
    </header>
  );
}

export function ContextUsageIndicator({
  usage,
  agentUsage,
  models,
  maxContextTokens,
}: {
  usage?: ContextUsage;
  agentUsage?: AgentUsageStatus;
  models: Model[];
  maxContextTokens: number;
}) {
  const contextTokens = Math.max(0, usage?.contextTokens ?? 0);
  const effectiveMax =
    maxContextTokens > 0 ? maxContextTokens : (usage?.maxContextTokens ?? 0);
  const ratio = effectiveMax > 0 ? contextTokens / effectiveMax : 0;
  const progress = Math.min(1, Math.max(0, ratio));
  const percent = effectiveMax > 0 ? Math.round(ratio * 100) : undefined;
  const level = ratio >= 0.85 ? "critical" : ratio >= 0.7 ? "warning" : "";
  const modelUsages = Object.entries(agentUsage?.byModel ?? {});
  const hasTokenUsage = Boolean(
    agentUsage?.currentTurn || agentUsage?.total || modelUsages.length,
  );

  return (
    <div
      className={`context-usage ${level}`}
      tabIndex={0}
      aria-label={
        percent === undefined
          ? t("context.unknownLimit")
          : t("context.usedPercentAria", { percent })
      }
    >
      <span className="context-usage-meter" aria-hidden="true">
        <svg viewBox="0 0 20 20">
          <circle className="context-usage-track" cx="10" cy="10" r="7.5" />
          <circle
            className="context-usage-progress"
            cx="10"
            cy="10"
            r="7.5"
            pathLength="100"
            strokeDasharray="100"
            strokeDashoffset={100 - progress * 100}
          />
        </svg>
      </span>
      <div className="context-usage-tooltip" role="tooltip">
        <section className="agent-token-usage" aria-label={t("usage.tokenUsage")}>
          <div className="usage-section-heading">
            <strong>{t("usage.tokenUsage")}</strong>
            <small>
              {modelUsages.length > 0
                ? t("usage.modelCount", { count: modelUsages.length })
                : t("usage.currentAgent")}
            </small>
          </div>
          {hasTokenUsage ? (
            <>
              <TokenUsageBreakdown
                label={t("usage.thisTurn")}
                usage={agentUsage?.currentTurn}
              />
              <TokenUsageBreakdown
                label={t("usage.sessionTotal")}
                usage={agentUsage?.total}
              />
              {modelUsages.length > 0 && (
                <div className="token-usage-models">
                  <strong>{t("usage.byModel")}</strong>
                  {modelUsages.slice(0, 3).map(([model, modelUsage]) => {
                    const totalInput = inputTokenUsage(modelUsage);
                    const modelDisplayName =
                      models.find(
                        (candidate) =>
                          candidate.id === model || candidate.model === model,
                      )?.displayName ?? model;
                    return (
                      <div
                        key={model}
                        title={t("usage.modelTooltip", {
                          name: modelDisplayName,
                          cacheInput: formatTokenCount(modelUsage.inputCacheRead),
                          totalInput: formatTokenCount(totalInput),
                          output: formatTokenCount(modelUsage.output),
                          hitRate: formatCacheHitRate(modelUsage),
                        })}
                      >
                        <span>
                          <i>{modelDisplayName}</i>
                          <b>{t("usage.hitRate", { rate: formatCacheHitRate(modelUsage) })}</b>
                        </span>
                        <small>
                          {t("usage.cacheInput")}{" "}
                          {formatCompactTokenCount(modelUsage.inputCacheRead)}
                          <em>/</em>
                          {t("usage.totalInput")}{" "}
                          {formatCompactTokenCount(totalInput)}
                          <em>/</em>
                          {t("usage.output")}{" "}
                          {formatCompactTokenCount(modelUsage.output)}
                        </small>
                      </div>
                    );
                  })}
                  {modelUsages.length > 3 && (
                    <small>{t("usage.moreModels", { count: modelUsages.length - 3 })}</small>
                  )}
                </div>
              )}
            </>
          ) : (
            <span className="token-usage-empty">{t("usage.empty")}</span>
          )}
        </section>
        <span className="context-usage-divider" aria-hidden="true" />
        <section className="context-window-usage" aria-label={t("context.window")}>
          <div className="usage-section-heading">
            <strong>{t("context.window")}</strong>
          </div>
          <span className="context-usage-summary">
            {percent === undefined ? t("context.usageUnknown") : t("context.usedPercent", { percent })}
          </span>
          <span>
            {t("context.usedOf", {
              used: formatTokenCount(contextTokens),
              total: effectiveMax > 0 ? formatTokenCount(effectiveMax) : t("context.unknown"),
            })}
          </span>
        </section>
      </div>
    </div>
  );
}

export function TodoProgress({ todos }: { todos: readonly TodoItem[] }) {
  const completed = todos.filter((todo) => todo.status === "done").length;
  const activeIndex = todos.findIndex(
    (todo) => todo.status === "in_progress",
  );
  const pendingIndex = todos.findIndex((todo) => todo.status === "pending");
  const currentIndex =
    activeIndex >= 0
      ? activeIndex
      : pendingIndex >= 0
        ? pendingIndex
        : Math.max(0, todos.length - 1);
  const allDone = completed === todos.length;
  const progressLabel = allDone
    ? t("todo.doneCount", { completed, total: todos.length })
    : t("todo.stepCount", { current: currentIndex + 1, total: todos.length });

  return (
    <div
      className={`todo-progress-anchor ${allDone ? "complete" : ""}`}
      tabIndex={0}
      aria-label={t("todo.ariaLabel", { label: progressLabel })}
    >
      <div className="todo-popover" role="tooltip">
        <div className="todo-popover-heading">
          <strong>{t("todo.current")}</strong>
          <span>
            {completed} / {todos.length} {t("status.completed")}
          </span>
        </div>
        <ol className="todo-list">
          {todos.map((todo, index) => (
            <li
              className={`todo-list-item ${todo.status}`}
              key={`${index}-${todo.title}`}
            >
              <span className="todo-status-mark" aria-hidden="true">
                {todo.status === "done" && <Check size={10} strokeWidth={2.4} />}
              </span>
              <span>{todo.title}</span>
            </li>
          ))}
        </ol>
      </div>
      <div className="todo-progress-pill" aria-hidden="true">
        <span className="todo-progress-ring">
          {allDone && <Check size={9} strokeWidth={2.6} />}
        </span>
        <span>{progressLabel}</span>
      </div>
    </div>
  );
}

function backgroundTaskStatusLabel(status: AgentTaskInfo["status"]): string {
  switch (status) {
    case "running":
      return t("status.running");
    case "completed":
      return t("status.completed");
    case "failed":
      return t("status.failed");
    case "timed_out":
      return t("status.timedOut");
    case "killed":
      return t("status.killed");
    case "lost":
      return t("status.lost");
  }
}

function backgroundTaskElapsed(task: AgentTaskInfo): string {
  const end = task.status === "running" ? Date.now() : task.endedAt;
  if (typeof end !== "number") return "";
  const duration = Math.max(0, end - task.startedAt);
  const seconds = Math.floor(duration / 1000);
  if (seconds < 60) return t("duration.seconds", { value: seconds });
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  if (minutes < 60) {
    return remainder > 0
      ? t("duration.minSec", { minutes, seconds: remainder })
      : t("duration.minutesTight", { minutes });
  }
  const hours = Math.floor(minutes / 60);
  return t("duration.hourMin", { hours, minutes: minutes % 60 });
}

export function BackgroundTaskProgress({
  tasks,
  onLoadOutput,
}: {
  tasks: readonly BackgroundTaskView[];
  onLoadOutput: (taskId: string) => Promise<void>;
}) {
  const [expandedTaskId, setExpandedTaskId] = useState<string>();
  const running = tasks.filter((task) => task.status === "running").length;
  const failed = tasks.filter((task) =>
    ["failed", "timed_out", "lost"].includes(task.status),
  ).length;
  const allDone = running === 0 && failed === 0;

  const toggleTask = (task: BackgroundTaskView): void => {
    if (expandedTaskId === task.taskId) {
      setExpandedTaskId(undefined);
      return;
    }
    setExpandedTaskId(task.taskId);
    void onLoadOutput(task.taskId);
  };

  return (
    <div
      className={`background-task-anchor ${allDone ? "complete" : ""}`}
      tabIndex={0}
      aria-label={t("tasks.ariaLabel", { count: tasks.length })}
    >
      <div
        className="background-task-popover"
        role="dialog"
        aria-label={t("tasks.title")}
      >
        <div className="background-task-popover-heading">
          <strong>{t("tasks.title")}</strong>
          <span>
            {running > 0
              ? t("tasks.runningCount", { count: running })
              : failed > 0
                ? t("tasks.failedCount", { count: failed })
                : t("tasks.completedCount", { count: tasks.length })}
          </span>
        </div>
        <ul className="background-task-list">
          {tasks.map((task) => {
            const expanded = expandedTaskId === task.taskId;
            const elapsed = backgroundTaskElapsed(task);
            return (
              <li
                className={`background-task-item ${task.status} ${
                  expanded ? "expanded" : ""
                }`}
                key={task.taskId}
              >
                <button
                  className="background-task-summary"
                  type="button"
                  aria-expanded={expanded}
                  onClick={() => toggleTask(task)}
                >
                  <span
                    className={`background-task-status-mark ${task.status}`}
                    aria-hidden="true"
                  >
                    {task.status === "running" ? (
                      <span className="spinner" />
                    ) : task.status === "completed" ? (
                      <Check size={10} strokeWidth={2.5} />
                    ) : (
                      <X size={10} strokeWidth={2.5} />
                    )}
                  </span>
                  <span className="background-task-copy">
                    <strong>
                      {task.description || task.command || task.taskId}
                    </strong>
                    <small>
                      {backgroundTaskStatusLabel(task.status)}
                      {elapsed ? ` · ${elapsed}` : ""}
                    </small>
                  </span>
                  <ChevronRight
                    className="background-task-chevron"
                    size={13}
                    aria-hidden="true"
                  />
                </button>
                {expanded && (
                  <div className="background-task-detail">
                    <span>{t("tasks.command")}</span>
                    <pre className="background-task-command">
                      <code>{task.command || task.description}</code>
                    </pre>
                    <span>{t("tasks.output")}</span>
                    <pre className="background-task-output">
                      <code>
                        {task.output ||
                          (task.outputLoading
                            ? t("tasks.loadingOutput")
                            : task.outputError || t("tasks.noOutput"))}
                      </code>
                    </pre>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      </div>
      <div className="background-task-pill" aria-hidden="true">
        <TerminalSquare size={13} />
        <span>{t("tasks.title")} {tasks.length}</span>
        {running > 0 && <span className="background-task-running-dot" />}
      </div>
    </div>
  );
}

function formatCompactionTokenCount(value: number): string {
  const amount = Math.max(0, value);
  const compact = (scaled: number, suffix: string): string =>
    `${scaled.toFixed(1).replace(/\.0$/, "")}${suffix}`;
  if (amount >= 1_000_000) return compact(amount / 1_000_000, "m");
  if (amount >= 1_000) return compact(amount / 1_000, "k");
  return Math.round(amount).toLocaleString("en-US");
}

export function compactionTokenTransition(
  event?: CompactionEvent,
): string | undefined {
  if (
    event?.tokensBefore === undefined ||
    event.tokensAfter === undefined
  ) {
    return undefined;
  }
  return `${formatCompactionTokenCount(
    event.tokensBefore,
  )} → ${formatCompactionTokenCount(event.tokensAfter)} tokens`;
}

export function CompactionNotice({ event }: { event: CompactionEvent }) {
  const tokenTransition = compactionTokenTransition(event);

  return (
    <div
      className={`compaction-live-divider ${event.phase}`}
      role="status"
    >
      <span aria-hidden="true" />
      {event.phase === "started" && (
        <span className="spinner" aria-hidden="true" />
      )}
      <strong>
        {event.phase === "completed"
          ? t("compaction.completed")
          : event.phase === "cancelled"
            ? t("compaction.cancelled")
            : t("compaction.inProgress")}
        {event.phase === "completed" && tokenTransition
          ? t("compaction.tokens", { transition: tokenTransition })
          : ""}
      </strong>
      <span aria-hidden="true" />
    </div>
  );
}

export function ToolbarSelect({
  className = "",
  ariaLabel,
  icon,
  value,
  label,
  options,
  disabled = false,
  onChange,
}: {
  className?: string;
  ariaLabel: string;
  icon: ReactNode;
  value: string;
  label: string;
  options: ToolbarSelectOption[];
  disabled?: boolean;
  onChange: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent): void => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, [open]);

  return (
    <div
      className={`toolbar-select ${className} ${open ? "open" : ""}`}
      ref={rootRef}
      onKeyDown={(event) => {
        if (event.key === "Escape") setOpen(false);
      }}
    >
      <button
        type="button"
        className="toolbar-select-trigger"
        aria-label={ariaLabel}
        title={label}
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
      >
        {icon}
        <span>{label}</span>
        <ChevronDown size={13} />
      </button>
      {open && (
        <div className="toolbar-select-menu" role="listbox" aria-label={ariaLabel}>
          {options.map((option) => {
            const selected = option.value === value;
            return (
              <button
                type="button"
                role="option"
                aria-selected={selected}
                className={`${selected ? "selected" : ""} ${
                  option.danger ? "danger" : ""
                }`}
                key={option.value}
                onClick={() => {
                  onChange(option.value);
                  setOpen(false);
                }}
              >
                <span>
                  <strong>{option.label}</strong>
                  {option.description && <small>{option.description}</small>}
                </span>
                {selected && <Check size={14} />}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
