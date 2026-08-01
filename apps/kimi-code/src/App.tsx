import {
  type CSSProperties,
  type FormEvent,
  type KeyboardEvent,
  type MouseEvent,
  type ClipboardEvent,
  type ChangeEvent,
  Fragment,
  type ReactNode,
  isValidElement,
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Archive,
  ArrowUp,
  Bot,
  BrainCircuit,
  Check,
  ChevronDown,
  ChevronRight,
  CircleUserRound,
  ClipboardList,
  Code2,
  Copy,
  ExternalLink,
  FileCode2,
  Folder,
  FolderGit2,
  FolderMinus,
  LogIn,
  LogOut,
  Menu,
  MessageSquareText,
  Minus,
  Minimize2,
  MoreHorizontal,
  Package,
  Paperclip,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  RefreshCw,
  Settings as SettingsIcon,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
  Square,
  SquarePen,
  TerminalSquare,
  Wrench,
  X,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import {
  archiveSession,
  createAgentClient,
  createOrTouchWorkspace,
  forkSession,
  getSkillContent,
  listSkills,
  listWorkspaceSessions,
  prepareSession,
  removeWorkspace,
  setDefaultModel,
  subscribeAgentEvents,
  type AgentPromptSubmitStatus,
  unsubscribeAgentEvents,
} from "./agentRpc";
import {
  conversationFromSession,
  conversationFromSummary,
  getActive,
  loadDesktopState,
  projectFromWorkspace,
} from "./store";
import { mergeDesktopInventory } from "./desktopInventory";
import {
  normalizeThinkingLevel,
  thinkingLevelDescription,
  thinkingLevelsForModel,
} from "./modelControls";
import { resolveMarkdownExternalUrl } from "./markdownLinks";
import {
  isSameLiveUserMessage,
  projectLiveUserMessage,
} from "./liveUserMessage";
import {
  canUndoPromptEdit,
  createPromptUndoHistory,
  recordPromptInput,
  undoPromptEdit,
} from "./promptUndo";
import {
  applyColorScheme,
  loadColorScheme,
  saveColorScheme,
  type ColorScheme,
} from "./appearance";
import SettingsDialog from "./SettingsDialog";
import { resolveAccountMenuVisibility } from "./accountMenu";
import {
  TRANSPORT_AUTH_REQUIRED,
  TRANSPORT_REPLAY_RESET,
  getAppVersion,
  invoke,
  isDesktop,
  listen,
  openExternalUrl,
  pickNativeDirectory,
  setWebCredential,
  uploadFileTransport,
  webCredentialRequired,
  type ReplayResetEvent,
} from "./transport";
import {
  MOBILE_LAYOUT_MAX_WIDTH,
  MOBILE_LAYOUT_QUERY,
  resolveSidebarCollapsed,
  shouldUseWebMobileLayout,
} from "./responsive";
import {
  applyLanguage,
  loadLanguage,
  localeTag,
  saveLanguage,
  setLanguage,
  t,
  type Language,
} from "./i18n";
import {
  isSubagentEvent,
  mergeSessionSubagentEvent,
  subagentRunsWithSwarmItems,
  type SessionSubagentRuns,
  type SubagentRun,
  type SubagentRunStatus,
  type SubagentRunsByTool,
} from "./subagentEvents";
import type {
  AccountUsage,
  AgentChatEvent,
  AgentChatEventEnvelope,
  AgentContentPart,
  AgentInteraction,
  AgentInteractionsEvent,
  AgentTaskInfo,
  AgentUsageStatus,
  ApprovalPayload,
  AuthStatus,
  BackgroundTaskView,
  CompactionEvent,
  ContextUsage,
  DesktopState,
  DeviceCode,
  MessageContent,
  LiveUserMessage,
  PromptSubmittedEvent,
  MessagePage,
  ManagedUsageRow,
  Model,
  AgentPromptPart,
  PermissionMode,
  PlanData,
  PlanReviewDisplay,
  Project,
  ProtocolMessage,
  QuestionPayload,
  QuestionResponse,
  SkillContent,
  SkillDescriptor,
  TokenUsage,
  TodoItem,
  ToolUpdate,
} from "./types";

const MAX_PROMPT_ATTACHMENTS = 8;
const MAX_PROMPT_SKILLS = 8;
const SLASH_COMMAND_COUNT = 3;
const MAX_PROMPT_ATTACHMENT_BYTES = 20 * 1024 * 1024;
const MAX_PROMPT_IMAGE_DIMENSION = 2048;
const IMAGE_COMPRESSION_THRESHOLD = 4 * 1024 * 1024;
const MAX_LIVE_TOOL_UPDATES = 50;
const LIVE_TURN_HANDOFF_MS = 200;
const BACKGROUND_TASK_LIST_LIMIT = 50;
const BACKGROUND_TASK_OUTPUT_TAIL = 16_384;
const BACKGROUND_TASK_DETAIL_TAIL = 65_536;
const MAIN_AGENT_ID = "main";
const PROMPT_IMAGE_TYPES = new Set([
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
]);
const PROMPT_AUDIO_TYPES = new Set([
  "audio/mpeg",
  "audio/mp3",
  "audio/wav",
  "audio/x-wav",
  "audio/ogg",
  "audio/webm",
  "audio/mp4",
]);
const PROMPT_VIDEO_TYPES = new Set([
  "video/mp4",
  "video/mpeg",
  "video/quicktime",
  "video/webm",
  "video/x-matroska",
  "video/x-msvideo",
  "video/3gpp",
]);

type RenderMessage = ProtocolMessage & {
  status?: "streaming" | "done" | "error";
};

interface ConversationHistory {
  conversationId: string;
  items: ProtocolMessage[];
  loading: boolean;
  error?: string;
}

interface ConversationOutlineItem {
  id: string;
  title: string;
  previewLines: string[];
  tickWidth: number;
}

type RemovalTarget =
  | {
      kind: "project";
      projectId: string;
      name: string;
      path: string;
      conversationIds: string[];
    }
  | {
      kind: "conversation";
      projectId: string;
      conversationId: string;
      title: string;
    };

type LiveTurnStatus =
  | "queued"
  | "running"
  | "completed"
  | "cancelled"
  | "failed"
  | "blocked";

type LiveStepStatus = "running" | "completed" | "interrupted";

type LiveBlock =
  | { kind: "text"; content: string }
  | { kind: "thinking"; content: string }
  | { kind: "content"; content: AgentContentPart }
  | {
      kind: "tool";
      toolCallId: string;
      name?: string;
      argumentsText: string;
      input?: unknown;
      description?: string;
      display?: unknown;
      status: "streaming" | "running" | "completed" | "error";
      updates: ToolUpdate[];
      output?: unknown;
      isError?: boolean;
    };

interface LiveStep {
  step: number;
  stepId?: string;
  status: LiveStepStatus;
  blocks: LiveBlock[];
  finishReason?: string;
  interruption?: string;
}

interface InFlightTurn {
  promptId?: string;
  userMessageId?: string;
  prompt: string;
  attachments: readonly PromptAttachment[];
  skills: readonly string[];
  steeredPrompts: readonly LiveSteeredPrompt[];
  createdAt: string;
  turnId?: number;
  status: LiveTurnStatus;
  durationMs?: number;
  steps: LiveStep[];
  error?: string;
  historyBoundaryId?: string;
}

interface QueuedAgentChatEvent {
  sessionId: string;
  agentId: string;
  event: AgentChatEvent;
}

type SubagentLiveTurns = Record<string, Record<string, InFlightTurn>>;

type PromptAttachmentKind = "image" | "audio" | "video" | "file";

interface PromptAttachment {
  id: string;
  name: string;
  dataUrl?: string;
  kind: PromptAttachmentKind;
  fileId?: string;
  mediaType: string;
  size: number;
}

interface QueuedPrompt {
  id: string;
  text: string;
  attachments: readonly PromptAttachment[];
  skills: readonly SkillDescriptor[];
  createdAt: string;
  steering?: boolean;
}

interface RemoteQueuedPrompt {
  promptId: string;
  userMessageId: string;
  text: string;
  attachments: readonly PromptAttachment[];
  skills: readonly string[];
  createdAt: string;
}

interface FolderHome {
  home: string;
  recent_roots: string[];
}

interface FolderBrowse {
  path: string;
  parent: string | null;
  entries: Array<{ name: string; path: string; is_dir: true }>;
}

interface LiveSteeredPrompt {
  promptId: string;
  message?: QueuedPrompt;
  anchorStepKey?: string;
  afterBlockIndex?: number;
}

interface UploadedFileMeta {
  id: string;
  name: string;
  media_type: string;
  size: number;
}

function promptAttachmentKind(
  mimeType: string,
): PromptAttachmentKind {
  if (PROMPT_IMAGE_TYPES.has(mimeType)) return "image";
  if (PROMPT_AUDIO_TYPES.has(mimeType)) return "audio";
  if (PROMPT_VIDEO_TYPES.has(mimeType)) return "video";
  return "file";
}

function readFileAsDataUrl(file: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () =>
      typeof reader.result === "string"
        ? resolve(reader.result)
        : reject(new Error(t("error.readMedia")));
    reader.onerror = () =>
      reject(reader.error ?? new Error(t("error.readMedia")));
    reader.readAsDataURL(file);
  });
}

function canvasToBlob(
  canvas: HTMLCanvasElement,
  type: string,
  quality?: number,
): Promise<Blob> {
  return new Promise((resolve, reject) => {
    canvas.toBlob(
      (blob) =>
        blob ? resolve(blob) : reject(new Error(t("error.processImage"))),
      type,
      quality,
    );
  });
}

async function preparePromptAttachment(file: File): Promise<PromptAttachment> {
  const kind = promptAttachmentKind(file.type);
  if (file.size > MAX_PROMPT_ATTACHMENT_BYTES) {
    throw new Error(t("error.fileTooLarge", { name: file.name }));
  }

  if (kind === "file") {
    const uploaded = (await uploadFileTransport(
      file,
      file.name || "attachment",
    )) as UploadedFileMeta;
    return {
      id: uploaded.id,
      fileId: uploaded.id,
      name: uploaded.name,
      mediaType: uploaded.media_type,
      size: uploaded.size,
      kind,
    };
  }

  let payload: Blob = file;
  if (kind === "image" && file.type !== "image/gif") {
    const bitmap = await createImageBitmap(file);
    try {
      const scale = Math.min(
        1,
        MAX_PROMPT_IMAGE_DIMENSION / Math.max(bitmap.width, bitmap.height),
      );
      if (scale < 1 || file.size > IMAGE_COMPRESSION_THRESHOLD) {
        const canvas = document.createElement("canvas");
        canvas.width = Math.max(1, Math.round(bitmap.width * scale));
        canvas.height = Math.max(1, Math.round(bitmap.height * scale));
        const context = canvas.getContext("2d", { alpha: false });
        if (!context) throw new Error(t("error.processImage"));
        context.fillStyle = "#ffffff";
        context.fillRect(0, 0, canvas.width, canvas.height);
        context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
        payload = await canvasToBlob(canvas, "image/jpeg", 0.86);
      }
    } finally {
      bitmap.close();
    }
  }

  return {
    id:
      typeof crypto.randomUUID === "function"
        ? crypto.randomUUID()
        : `${Date.now()}-${Math.random().toString(36).slice(2)}`,
    name: file.name || `clipboard-${kind}`,
    dataUrl: await readFileAsDataUrl(payload),
    mediaType: payload.type || file.type || "application/octet-stream",
    size: payload.size,
    kind,
  };
}

function buildAgentPromptInput(
  text: string,
  attachments: readonly PromptAttachment[],
): AgentPromptPart[] {
  return [
    ...(text ? [{ type: "text" as const, text }] : []),
    ...attachments.map((attachment): AgentPromptPart => {
      switch (attachment.kind) {
        case "image":
          return {
            type: "image_url",
            imageUrl: { url: attachment.dataUrl!, id: attachment.id },
          };
        case "audio":
          return {
            type: "audio_url",
            audioUrl: { url: attachment.dataUrl!, id: attachment.id },
          };
        case "video":
          return {
            type: "video_url",
            videoUrl: { url: attachment.dataUrl!, id: attachment.id },
          };
        case "file":
          return {
            type: "file",
            file_id: attachment.fileId!,
            name: attachment.name,
            media_type: attachment.mediaType,
            size: attachment.size,
          };
      }
    }),
  ];
}

function buildSkillPromptText(
  text: string,
  skills: readonly SkillDescriptor[],
): string {
  const mentions = skills.map((skill) => `$${skill.name}`).join(" ");
  return [mentions, text].filter(Boolean).join(" ");
}

interface SkillPromptDisplay {
  text: string;
  skills: string[];
}

interface SkillDetailTarget {
  name: string;
  description?: string;
  source?: SkillDescriptor["source"];
}

interface CompactionSummaryDetail {
  id: string;
  content: string;
  createdAt: string;
}

interface SideChatState {
  instanceId: number;
  parentSessionId: string;
  agentId?: string;
  draft: string;
  turns: InFlightTurn[];
  starting: boolean;
}

function decodeSkillAttribute(value: string): string {
  return value
    .replaceAll("&quot;", '"')
    .replaceAll("&apos;", "'")
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&amp;", "&");
}

function parseSkillPromptDisplay(value: string): SkillPromptDisplay {
  const skills: string[] = [];
  const collectSkills = (content: string): void => {
    const pattern =
      /<kimi-skill-loaded\b[^>]*\bname=(["'])(.*?)\1[^>]*>/gi;
    for (const match of content.matchAll(pattern)) {
      const name = decodeSkillAttribute(match[2]).trim();
      if (name && !skills.includes(name)) skills.push(name);
    }
  };

  let text = value.replace(
    /<kimi-selected-skills\b[^>]*>[\s\S]*?<\/kimi-selected-skills>\s*/gi,
    (block) => {
      collectSkills(block);
      return "";
    },
  );
  text = text.replace(
    /<kimi-skill-loaded\b[^>]*>[\s\S]*?<\/kimi-skill-loaded>\s*/gi,
    (block) => {
      collectSkills(block);
      return "";
    },
  );
  text = text.replace(
    /User activated the skill "[^"]+"\.\s*Follow the loaded skill instructions\.\s*/gi,
    "",
  );
  return {
    text: text.trim().replace(/\n{3,}/g, "\n\n"),
    skills,
  };
}

function SkillNameChips({
  names,
  onSkillOpen,
}: {
  names: readonly string[];
  onSkillOpen?: (name: string) => void;
}) {
  if (names.length === 0) return null;
  return (
    <div className="message-skill-list" aria-label={t("skills.usedInMessage")}>
      {names.map((name) =>
        onSkillOpen ? (
          <button
            className="message-skill-chip"
            type="button"
            title={t("skills.viewDetail")}
            aria-label={t("skills.viewSkill", { name })}
            key={name}
            onClick={() => onSkillOpen(name)}
          >
            <Package size={13} />
            {name}
          </button>
        ) : (
          <span className="message-skill-chip" key={name}>
            <Package size={13} />
            {name}
          </span>
        ),
      )}
    </div>
  );
}

function SkillPromptDisplayContent({
  text,
  skills = [],
  onSkillOpen,
}: {
  text: string;
  skills?: readonly string[];
  onSkillOpen?: (name: string) => void;
}) {
  const parsed = parseSkillPromptDisplay(text);
  const names = [...skills];
  for (const name of parsed.skills) {
    if (!names.includes(name)) names.push(name);
  }
  return (
    <>
      <SkillNameChips names={names} onSkillOpen={onSkillOpen} />
      {parsed.text}
    </>
  );
}

function newQueuedPromptId(): string {
  return typeof crypto.randomUUID === "function"
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

interface AgentSubscription {
  agentId: string;
  subscriptionId: string;
}

interface PendingAgentSubscription {
  agentId: string;
  promise: Promise<string>;
}

type ToolResultContent = Extract<MessageContent, { type: "tool_result" }>;

interface HistoryToolPresentation {
  messages: ProtocolMessage[];
  results: Map<string, ToolResultContent>;
}

interface HistoryConversationTurn {
  id: string;
  user?: RenderMessage;
  responses: RenderMessage[];
}

const PROMPT_SUGGESTIONS = [
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

function formatTime(timestamp: string | number): string {
  return new Intl.DateTimeFormat(localeTag(), {
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(timestamp));
}

function formatContext(value: number): string {
  if (value >= 1_000_000) return `${Math.round(value / 1_000_000)}M`;
  if (value >= 1_000) return `${Math.round(value / 1_000)}K`;
  return `${value}`;
}

function formatBytes(value: number): string {
  if (value >= 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB`;
  if (value >= 1024) return `${Math.ceil(value / 1024)} KB`;
  return `${value} B`;
}

function formatTokenCount(value: number): string {
  return Math.max(0, Math.round(value)).toLocaleString("en-US");
}

function formatCompactTokenCount(value: number): string {
  return new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(Math.max(0, Math.round(value)));
}

function inputTokenUsage(usage?: TokenUsage): number {
  if (!usage) return 0;
  return (
    usage.inputOther +
    usage.inputCacheRead +
    usage.inputCacheCreation
  );
}

function formatCacheHitRate(usage?: TokenUsage): string {
  if (!usage) return "—";
  const totalInput = inputTokenUsage(usage);
  if (totalInput <= 0) return "0%";
  return `${Math.round((usage.inputCacheRead / totalInput) * 100)}%`;
}

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

function conciseError(error: unknown): string {
  const message =
    error instanceof Error
      ? error.message
      : error &&
          typeof error === "object" &&
          "message" in error &&
          typeof error.message === "string"
        ? error.message
        : String(error);
  const summary =
    message
      .split(/\r?\n/)
      .map((line) => line.trim())
      .find(Boolean) ?? "Unknown error";
  const cleaned = summary.replace(/^Error:\s*/i, "");
  return cleaned.length > 300 ? `${cleaned.slice(0, 297)}...` : cleaned;
}

function fetchConversationHistory(
  conversationId: string,
): Promise<MessagePage> {
  return invoke<MessagePage>("list_conversation_messages", {
    sessionId: conversationId,
  });
}

function newInFlightTurn(
  prompt: string,
  attachments: readonly PromptAttachment[],
  historyBoundaryId?: string,
  skills: readonly string[] = [],
): InFlightTurn {
  return {
    prompt,
    attachments,
    skills,
    steeredPrompts: [],
    createdAt: new Date().toISOString(),
    status: "queued",
    steps: [],
    historyBoundaryId,
  };
}

function inFlightTurnFromUserMessage(message: LiveUserMessage): InFlightTurn {
  const projected = projectLiveUserMessage(message);
  const display = parseSkillPromptDisplay(projected.text);
  return {
    ...newInFlightTurn(
      display.text,
      projected.attachments,
      undefined,
      display.skills,
    ),
    promptId: message.promptId,
    userMessageId: message.userMessageId,
    createdAt: message.createdAt,
  };
}

function readPromptSubmittedEvent(
  event: { type: string; [key: string]: unknown },
): PromptSubmittedEvent | undefined {
  if (
    event.type !== "prompt.submitted" ||
    event.status !== "queued" ||
    typeof event.promptId !== "string" ||
    typeof event.userMessageId !== "string" ||
    typeof event.createdAt !== "string" ||
    !Array.isArray(event.content)
  ) {
    return undefined;
  }
  return event as unknown as PromptSubmittedEvent;
}

function isTurnRunning(turn?: InFlightTurn): boolean {
  return turn?.status === "queued" || turn?.status === "running";
}

function liveStepKey(step: number, stepId?: string): string {
  return stepId ?? `step-${step}`;
}

function liveTurnStatusFromSubmit(
  status: AgentPromptSubmitStatus,
): LiveTurnStatus {
  switch (status) {
    case "queued":
      return "queued";
    case "running":
    case "steered":
      return "running";
    case "completed":
      return "completed";
    case "failed":
      return "failed";
    case "cancelled":
      return "cancelled";
    case "blocked":
      return "blocked";
  }
}

function withCurrentStep(
  turn: InFlightTurn,
  update: (step: LiveStep) => LiveStep,
): InFlightTurn {
  const steps =
    turn.steps.length > 0
      ? [...turn.steps]
      : [{ step: 0, status: "running" as const, blocks: [] }];
  const index = steps.length - 1;
  steps[index] = update(steps[index]);
  return { ...turn, steps };
}

function appendLiveContent(
  turn: InFlightTurn,
  kind: "text" | "thinking",
  content: string,
): InFlightTurn {
  return withCurrentStep(turn, (step) => {
    const blocks = [...step.blocks];
    const last = blocks.at(-1);
    const lastIndex = blocks.length - 1;
    const stepKey = liveStepKey(step.step, step.stepId);
    const hasSteeredBoundary = turn.steeredPrompts.some(
      (item) =>
        item.anchorStepKey === stepKey &&
        item.afterBlockIndex === lastIndex,
    );
    if (last?.kind === kind && !hasSteeredBoundary) {
      blocks[blocks.length - 1] = {
        ...last,
        content: last.content + content,
      };
    } else {
      blocks.push({ kind, content });
    }
    return { ...step, blocks };
  });
}

function updateLiveTool(
  turn: InFlightTurn,
  toolCallId: string,
  update: (tool: Extract<LiveBlock, { kind: "tool" }>) => LiveBlock,
): InFlightTurn {
  for (
    let stepIndex = turn.steps.length - 1;
    stepIndex >= 0;
    stepIndex -= 1
  ) {
    const step = turn.steps[stepIndex];
    const blockIndex = step.blocks.findIndex(
      (block) => block.kind === "tool" && block.toolCallId === toolCallId,
    );
    if (blockIndex >= 0) {
      const block = step.blocks[blockIndex];
      if (block.kind === "tool") {
        const blocks = [...step.blocks];
        blocks[blockIndex] = update(block);
        const steps = [...turn.steps];
        steps[stepIndex] = { ...step, blocks };
        return { ...turn, steps };
      }
    }
  }

  return withCurrentStep(turn, (step) => ({
    ...step,
    blocks: [
      ...step.blocks,
      update({
        kind: "tool",
        toolCallId,
        argumentsText: "",
        status: "streaming",
        updates: [],
      }),
    ],
  }));
}

function reduceAgentChatEvent(
  turn: InFlightTurn,
  event: AgentChatEvent,
): InFlightTurn {
  if (event.type === "prompt.steered") {
    const known = new Set(turn.steeredPrompts.map((item) => item.promptId));
    const currentStep = turn.steps.at(-1);
    const placement = currentStep
      ? {
          anchorStepKey: liveStepKey(currentStep.step, currentStep.stepId),
          afterBlockIndex: currentStep.blocks.length - 1,
        }
      : {};
    const additions = event.promptIds
      .filter((promptId) => !known.has(promptId))
      .map((promptId) => ({ promptId, ...placement }));
    return additions.length > 0
      ? {
          ...turn,
          steeredPrompts: [...turn.steeredPrompts, ...additions],
        }
      : turn;
  }
  if (turn.turnId !== undefined && turn.turnId !== event.turnId) return turn;
  const next =
    turn.turnId === undefined ? { ...turn, turnId: event.turnId } : turn;

  switch (event.type) {
    case "turn.started": {
      const projected = event.userMessage
        ? inFlightTurnFromUserMessage(event.userMessage)
        : undefined;
      return {
        ...next,
        ...(projected
          ? {
              promptId: projected.promptId,
              userMessageId: projected.userMessageId,
              prompt: projected.prompt,
              attachments: projected.attachments,
              skills: projected.skills,
              createdAt: projected.createdAt,
            }
          : {}),
        status: "running",
      };
    }
    case "turn.ended":
      return {
        ...next,
        status: event.reason,
        durationMs:
          event.durationMs ??
          Math.max(0, Date.now() - Date.parse(next.createdAt)),
        error:
          event.reason === "failed" && event.error !== undefined
            ? conciseError(
                typeof event.error === "string"
                  ? event.error
                  : JSON.stringify(event.error),
              )
            : next.error,
      };
    case "turn.step.started": {
      const anchorStepKey = liveStepKey(event.step, event.stepId);
      const steeredPrompts = next.steeredPrompts.map((item) =>
        item.anchorStepKey === undefined
          ? { ...item, anchorStepKey, afterBlockIndex: -1 }
          : item,
      );
      const index = next.steps.findIndex(
        (step) =>
          (event.stepId && step.stepId === event.stepId) ||
          step.step === event.step,
      );
      if (index < 0) {
        return {
          ...next,
          status: "running",
          steeredPrompts,
          steps: [
            ...next.steps,
            {
              step: event.step,
              stepId: event.stepId,
              status: "running",
              blocks: [],
            },
          ],
        };
      }
      const steps = [...next.steps];
      steps[index] = { ...steps[index], status: "running" };
      return { ...next, status: "running", steeredPrompts, steps };
    }
    case "turn.step.completed": {
      const steps = next.steps.map((step) =>
        step.step === event.step
          ? {
              ...step,
              status: "completed" as const,
              finishReason: event.finishReason,
            }
          : step,
      );
      return { ...next, steps };
    }
    case "turn.step.interrupted": {
      const steps = next.steps.map((step) =>
        step.step === event.step
          ? {
              ...step,
              status: "interrupted" as const,
              interruption: event.message ?? event.reason,
            }
          : step,
      );
      return { ...next, steps };
    }
    case "assistant.delta":
      return appendLiveContent(next, "text", event.delta);
    case "assistant.content":
      return withCurrentStep(next, (step) => ({
        ...step,
        blocks: [...step.blocks, { kind: "content", content: event.content }],
      }));
    case "thinking.delta":
      return appendLiveContent(next, "thinking", event.delta);
    case "tool.call.delta":
      return updateLiveTool(next, event.toolCallId, (tool) => ({
        ...tool,
        name: event.name ?? tool.name,
        argumentsText: tool.argumentsText + (event.argumentsPart ?? ""),
      }));
    case "tool.call.started":
      return updateLiveTool(next, event.toolCallId, (tool) => ({
        ...tool,
        name: event.name,
        input: event.args,
        description: event.description,
        display: event.display,
        status: "running",
      }));
    case "tool.progress":
      return updateLiveTool(next, event.toolCallId, (tool) => ({
        ...tool,
        status: "running",
        updates: [
          ...tool.updates.slice(-(MAX_LIVE_TOOL_UPDATES - 1)),
          event.update,
        ],
      }));
    case "tool.result":
      return updateLiveTool(next, event.toolCallId, (tool) => ({
        ...tool,
        status: event.isError ? "error" : "completed",
        output: event.output,
        isError: event.isError,
      }));
  }
}

function appendQueuedAgentChatEvent(
  events: AgentChatEvent[],
  event: AgentChatEvent,
): void {
  const previous = events.at(-1);
  if (
    previous?.type === "assistant.delta" &&
    event.type === "assistant.delta" &&
    previous.turnId === event.turnId
  ) {
    events[events.length - 1] = {
      ...event,
      delta: previous.delta + event.delta,
    };
    return;
  }
  if (
    previous?.type === "thinking.delta" &&
    event.type === "thinking.delta" &&
    previous.turnId === event.turnId
  ) {
    events[events.length - 1] = {
      ...event,
      delta: previous.delta + event.delta,
    };
    return;
  }
  if (
    previous?.type === "tool.call.delta" &&
    event.type === "tool.call.delta" &&
    previous.turnId === event.turnId &&
    previous.toolCallId === event.toolCallId
  ) {
    events[events.length - 1] = {
      ...event,
      name: event.name ?? previous.name,
      argumentsPart:
        (previous.argumentsPart ?? "") + (event.argumentsPart ?? ""),
    };
    return;
  }
  events.push(event);
}

function reduceQueuedAgentChatEvents(
  current: Record<string, InFlightTurn>,
  queue: QueuedAgentChatEvent[],
): Record<string, InFlightTurn> {
  const eventsBySession = new Map<string, AgentChatEvent[]>();
  for (const queued of queue) {
    const events = eventsBySession.get(queued.sessionId) ?? [];
    appendQueuedAgentChatEvent(events, queued.event);
    eventsBySession.set(queued.sessionId, events);
  }

  let next = current;
  for (const [sessionId, events] of eventsBySession) {
    const turn = current[sessionId];
    let reduced = turn;
    for (const event of events) {
      if (!reduced) {
        if (event.type !== "turn.started") continue;
        reduced = newSubagentTurn(event);
      }
      if (
        event.type === "turn.started" &&
        reduced.turnId !== undefined &&
        reduced.turnId !== event.turnId &&
        !isTurnRunning(reduced)
      ) {
        reduced = newSubagentTurn(event);
      }
      reduced = reduceAgentChatEvent(reduced, event);
    }
    if (!reduced) continue;
    if (reduced === turn) continue;
    if (next === current) next = { ...current };
    next[sessionId] = reduced;
  }
  return next;
}

function newSubagentTurn(event: AgentChatEvent): InFlightTurn {
  if (event.type === "turn.started" && event.userMessage) {
    return inFlightTurnFromUserMessage(event.userMessage);
  }
  return newInFlightTurn(
    event.type === "turn.started" ? event.prompt ?? "" : "",
    [],
  );
}

function reduceQueuedSubagentChatEvents(
  current: SubagentLiveTurns,
  queue: QueuedAgentChatEvent[],
): SubagentLiveTurns {
  const grouped = new Map<
    string,
    {
      sessionId: string;
      agentId: string;
      events: AgentChatEvent[];
    }
  >();
  for (const queued of queue) {
    const key = `${queued.sessionId}\u0000${queued.agentId}`;
    const entry = grouped.get(key) ?? {
      sessionId: queued.sessionId,
      agentId: queued.agentId,
      events: [],
    };
    appendQueuedAgentChatEvent(entry.events, queued.event);
    grouped.set(key, entry);
  }

  let next = current;
  for (const { sessionId, agentId, events } of grouped.values()) {
    const sessionTurns = next[sessionId] ?? {};
    const previous = sessionTurns[agentId];
    let reduced = previous;
    for (const event of events) {
      if (
        !reduced ||
        (event.type === "turn.started" &&
          reduced.turnId !== undefined &&
          reduced.turnId !== event.turnId)
      ) {
        reduced = newSubagentTurn(event);
      }
      reduced = reduceAgentChatEvent(reduced, event);
    }
    if (!reduced || reduced === previous) continue;
    if (next === current) next = { ...current };
    next[sessionId] = {
      ...sessionTurns,
      [agentId]: reduced,
    };
  }
  return next;
}

const CHAT_EVENT_TYPES = new Set([
  "prompt.steered",
  "turn.started",
  "turn.ended",
  "turn.step.started",
  "turn.step.completed",
  "turn.step.interrupted",
  "assistant.delta",
  "assistant.content",
  "thinking.delta",
  "tool.call.delta",
  "tool.call.started",
  "tool.progress",
  "tool.result",
]);

function isAgentChatEvent(event: { type: string }): event is AgentChatEvent {
  return CHAT_EVENT_TYPES.has(event.type);
}

function readTodoItems(value: unknown): TodoItem[] | undefined {
  if (!Array.isArray(value)) return undefined;
  const todos: TodoItem[] = [];
  for (const item of value) {
    if (
      !item ||
      typeof item !== "object" ||
      !("title" in item) ||
      typeof item.title !== "string" ||
      !("status" in item) ||
      !["pending", "in_progress", "done"].includes(String(item.status))
    ) {
      return undefined;
    }
    todos.push({
      title: item.title,
      status: item.status as TodoItem["status"],
    });
  }
  return todos;
}

const AGENT_TASK_STATUSES = new Set([
  "running",
  "completed",
  "failed",
  "timed_out",
  "killed",
  "lost",
]);

function readAgentTaskInfo(
  value: unknown,
  fallbackStatus?: AgentTaskInfo["status"],
): AgentTaskInfo | undefined {
  if (!value || typeof value !== "object") return undefined;
  const record = value as Record<string, unknown>;
  if (
    typeof record.taskId !== "string" ||
    typeof record.kind !== "string"
  ) {
    return undefined;
  }
  const status = AGENT_TASK_STATUSES.has(String(record.status))
    ? (record.status as AgentTaskInfo["status"])
    : fallbackStatus;
  if (!status) return undefined;

  return {
    ...(record as unknown as AgentTaskInfo),
    taskId: record.taskId,
    kind: record.kind,
    status,
    description:
      typeof record.description === "string"
        ? record.description
        : typeof record.command === "string"
          ? record.command
          : record.taskId,
    startedAt:
      typeof record.startedAt === "number" ? record.startedAt : Date.now(),
  };
}

function isTaskLifecycleEventType(type: string): boolean {
  return (
    type === "task.started" ||
    type === "task.terminated" ||
    type === "background.task.started" ||
    type === "background.task.terminated"
  );
}

function messageOriginKind(message: ProtocolMessage): string | undefined {
  const origin = message.metadata?.origin;
  return origin && typeof origin === "object" && "kind" in origin
    ? String(origin.kind)
    : undefined;
}

function isVisibleHistoryMessage(message: ProtocolMessage): boolean {
  return !["injection", "system_trigger", "task", "cron"].includes(
    messageOriginKind(message) ?? "",
  );
}

function historyBeforeInFlightTurn(
  items: ProtocolMessage[],
  turn: InFlightTurn,
): ProtocolMessage[] {
  if (turn.userMessageId) {
    const userMessage = items.findIndex(
      (message) => message.id === turn.userMessageId,
    );
    if (userMessage >= 0) return items.slice(0, userMessage);
  }
  if (turn.historyBoundaryId) {
    const boundary = items.findIndex(
      (message) => message.id === turn.historyBoundaryId,
    );
    if (boundary >= 0) return items.slice(0, boundary + 1);
  }

  const prompt = parseSkillPromptDisplay(turn.prompt).text;
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const message = items[index];
    if (message.role === "user" && displayMessageText(message) === prompt) {
      return items.slice(0, index);
    }
  }
  return items;
}

function completedTurnMessageId(
  items: ProtocolMessage[],
  turn: InFlightTurn,
): string | undefined {
  let startIndex = 0;
  if (turn.userMessageId) {
    const userMessage = items.findIndex(
      (message) => message.id === turn.userMessageId,
    );
    if (userMessage >= 0) startIndex = userMessage + 1;
  } else if (turn.historyBoundaryId) {
    const boundary = items.findIndex(
      (message) => message.id === turn.historyBoundaryId,
    );
    if (boundary >= 0) startIndex = boundary + 1;
  } else {
    for (let index = items.length - 1; index >= 0; index -= 1) {
      const message = items[index];
      if (
        message.role === "user" &&
        displayMessageText(message) ===
          parseSkillPromptDisplay(turn.prompt).text
      ) {
        startIndex = index + 1;
        break;
      }
    }
  }

  for (let index = items.length - 1; index >= startIndex; index -= 1) {
    if (items[index].role === "assistant") return items[index].id;
  }
  return undefined;
}

function formatElapsedDuration(durationMs: number): string {
  const totalSeconds = Math.max(0, durationMs) / 1000;
  if (totalSeconds < 10)
    return t("duration.seconds", { value: totalSeconds.toFixed(1) });
  const roundedSeconds = Math.round(totalSeconds);
  if (roundedSeconds < 60)
    return t("duration.seconds", { value: roundedSeconds });
  const minutes = Math.floor(roundedSeconds / 60);
  const seconds = roundedSeconds % 60;
  return seconds > 0
    ? t("duration.minutesSeconds", { minutes, seconds })
    : t("duration.minutes", { value: minutes });
}

function mergeHistoryToolResults(
  messages: ProtocolMessage[],
): HistoryToolPresentation {
  const results = new Map<string, ToolResultContent>();

  for (const message of messages) {
    for (const part of message.content) {
      if (part.type === "tool_result") results.set(part.tool_call_id, part);
    }
  }

  const mergedMessages = messages.flatMap((message) => {
    const content = message.content.filter((part) => part.type !== "tool_result");
    if (content.length === 0) return [];
    return content.length === message.content.length
      ? [message]
      : [{ ...message, content }];
  });

  return { messages: mergedMessages, results };
}

function groupHistoryMessages(
  messages: ProtocolMessage[],
): HistoryConversationTurn[] {
  const turns: HistoryConversationTurn[] = [];

  for (const message of messages) {
    if (messageOriginKind(message) === "compaction_summary") {
      turns.push({
        id: message.id,
        responses: [message],
      });
      continue;
    }
    if (message.role === "user") {
      turns.push({
        id: message.prompt_id ?? message.id,
        user: message,
        responses: [],
      });
      continue;
    }

    let turn = turns.at(-1);
    if (
      !turn ||
      turn.responses.some(
        (response) =>
          messageOriginKind(response) === "compaction_summary",
      )
    ) {
      turn = {
        id: message.prompt_id ?? message.id,
        responses: [],
      };
      turns.push(turn);
    }
    turn.responses.push(message);
  }

  return turns;
}

function finalResponseMessage(
  messages: RenderMessage[],
): RenderMessage | undefined {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (
      message.role === "assistant" &&
      message.status !== "streaming" &&
      messageOriginKind(message) !== "compaction_summary" &&
      messageText(message).trim().length > 0
    ) {
      return message;
    }
  }
  return undefined;
}

function compactOutlineText(value: string, maxLength: number): string {
  const compact = value.trim().replace(/\s+/g, " ");
  if (compact.length <= maxLength) return compact;
  return `${compact.slice(0, Math.max(1, maxLength - 1)).trimEnd()}…`;
}

function conversationOutlinePreview(value: string): string[] {
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

function outlineTickWidth(messageLength: number): number {
  if (messageLength <= 0) return 6;
  return Math.min(15, 6 + Math.round(Math.log2(messageLength + 1) * 0.85));
}

function omitSessionKeys<T>(
  current: Record<string, T>,
  sessionIds: ReadonlySet<string>,
): Record<string, T> {
  let changed = false;
  const next = { ...current };
  for (const sessionId of sessionIds) {
    if (!(sessionId in next)) continue;
    delete next[sessionId];
    changed = true;
  }
  return changed ? next : current;
}

export default function App() {
  const desktopRuntime = useMemo(isDesktop, []);
  const [mobileQueryMatches, setMobileQueryMatches] = useState(() =>
    typeof window.matchMedia === "function"
      ? window.matchMedia(MOBILE_LAYOUT_QUERY).matches
      : window.innerWidth <= MOBILE_LAYOUT_MAX_WIDTH,
  );
  const mobileLayout = shouldUseWebMobileLayout(
    desktopRuntime,
    mobileQueryMatches,
  );
  const [desktop, setDesktop] = useState<DesktopState>({ projects: [] });
  const [auth, setAuth] = useState<AuthStatus>({
    loggedIn: false,
    provider: "kimi-code",
  });
  const [models, setModels] = useState<Model[]>([]);
  const [prompt, setPrompt] = useState("");
  const [promptAttachments, setPromptAttachments] = useState<
    PromptAttachment[]
  >([]);
  const [promptSkills, setPromptSkills] = useState<SkillDescriptor[]>([]);
  const [availableSkills, setAvailableSkills] = useState<SkillDescriptor[]>([]);
  const [skillsBusy, setSkillsBusy] = useState(false);
  const [skillsError, setSkillsError] = useState<string>();
  const [skillDetailTarget, setSkillDetailTarget] =
    useState<SkillDetailTarget>();
  const [skillDetail, setSkillDetail] = useState<SkillContent>();
  const [skillDetailBusy, setSkillDetailBusy] = useState(false);
  const [skillDetailError, setSkillDetailError] = useState<string>();
  const [compactionSummaryDetail, setCompactionSummaryDetail] =
    useState<CompactionSummaryDetail>();
  const [sideChat, setSideChat] = useState<SideChatState>();
  const [composerAddOpen, setComposerAddOpen] = useState(false);
  const [slashMenuOpen, setSlashMenuOpen] = useState(false);
  const [slashMenuActiveIndex, setSlashMenuActiveIndex] = useState(0);
  const [compactionCommandBusy, setCompactionCommandBusy] = useState(false);
  const [forkCommandBusy, setForkCommandBusy] = useState(false);
  const [queuedPrompts, setQueuedPrompts] = useState<
    Record<string, QueuedPrompt[]>
  >({});
  const [remoteQueuedPrompts, setRemoteQueuedPrompts] = useState<
    Record<string, RemoteQueuedPrompt[]>
  >({});
  const [desktopSidebarCollapsed, setDesktopSidebarCollapsed] =
    useState(false);
  const [mobileSidebarOpen, setMobileSidebarOpen] = useState(false);
  const [mobileViewportHeight, setMobileViewportHeight] = useState<number>();
  const [loginOpen, setLoginOpen] = useState(false);
  const [webAuthOpen, setWebAuthOpen] = useState(webCredentialRequired);
  const [directoryPickerOpen, setDirectoryPickerOpen] = useState(false);
  const [loginBusy, setLoginBusy] = useState(false);
  const [deviceCode, setDeviceCode] = useState<DeviceCode>();
  const [profileOpen, setProfileOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [colorScheme, setColorScheme] =
    useState<ColorScheme>(loadColorScheme);
  const [language, setLanguageState] = useState<Language>(loadLanguage);
  const [appVersion, setAppVersion] = useState<string>();
  const [accountUsage, setAccountUsage] = useState<AccountUsage>();
  const [accountUsageBusy, setAccountUsageBusy] = useState(false);
  const [accountUsageError, setAccountUsageError] = useState<string>();
  const [modelsBusy, setModelsBusy] = useState(false);
  const [modelBusy, setModelBusy] = useState(false);
  const [notice, setNotice] = useState<string>();
  const [copiedMessage, setCopiedMessage] = useState<string>();
  const [interactions, setInteractions] = useState<
    Record<string, AgentInteraction[]>
  >({});
  const [resolvingInteraction, setResolvingInteraction] = useState<string>();
  const [compactions, setCompactions] = useState<
    Record<string, CompactionEvent>
  >({});
  const [compactionHistoryReady, setCompactionHistoryReady] = useState<
    Record<string, boolean>
  >({});
  const [contextUsages, setContextUsages] = useState<
    Record<string, ContextUsage>
  >({});
  const [agentUsages, setAgentUsages] = useState<
    Record<string, AgentUsageStatus>
  >({});
  const [messageDurations, setMessageDurations] = useState<
    Record<string, Record<string, number>>
  >({});
  const [plans, setPlans] = useState<Record<string, PlanData | null>>({});
  const [sessionTodos, setSessionTodos] = useState<Record<string, TodoItem[]>>(
    {},
  );
  const [backgroundTasks, setBackgroundTasks] = useState<
    Record<string, BackgroundTaskView[]>
  >({});
  const [subagentRuns, setSubagentRuns] = useState<SessionSubagentRuns>({});
  const [subagentLiveTurns, setSubagentLiveTurns] =
    useState<SubagentLiveTurns>({});
  const [modeBusy, setModeBusy] = useState(false);
  const [removalTarget, setRemovalTarget] = useState<RemovalTarget>();
  const [removalBusy, setRemovalBusy] = useState(false);
  const [historyByConversation, setHistoryByConversation] = useState<
    Record<string, ConversationHistory>
  >({});
  const [activeOutlineTurnId, setActiveOutlineTurnId] = useState<string>();
  const [inFlightTurns, setInFlightTurns] = useState<
    Record<string, InFlightTurn>
  >({});
  const inFlightTurnsRef = useRef(inFlightTurns);
  const [activeAgentScope, setActiveAgentScope] = useState<{
    sessionId: string;
    agentId: string;
  }>();
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const promptUndoHistoryRef = useRef(createPromptUndoHistory());
  const promptUndoConversationRef = useRef<string | undefined>(undefined);
  const promptCompositionRef = useRef(false);
  const attachmentInputRef = useRef<HTMLInputElement>(null);
  const composerAddRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const messageStackRef = useRef<HTMLDivElement>(null);
  const mobileMenuButtonRef = useRef<HTMLButtonElement>(null);
  const followLatestMessageRef = useRef(true);
  const lastChatScrollTopRef = useRef(0);
  const lastChatScrollHeightRef = useRef(0);
  const chatScrollFrameRef = useRef<number | undefined>(undefined);
  const outlineScrollFrameRef = useRef<number | undefined>(undefined);
  const profileRef = useRef<HTMLDivElement>(null);
  const noticeTimer = useRef<number | undefined>(undefined);
  const accountUsageRequest = useRef(0);
  const historyRequests = useRef<Record<string, number>>({});
  const desktopInventoryRequest = useRef(0);
  const backgroundTaskRequests = useRef<Record<string, number>>({});
  const skillsRequest = useRef(0);
  const skillDetailRequest = useRef(0);
  const agentSubscriptions = useRef<Map<string, AgentSubscription>>(new Map());
  const pendingAgentSubscriptions = useRef<
    Map<string, PendingAgentSubscription>
  >(new Map());
  const queuedAgentChatEvents = useRef<QueuedAgentChatEvent[]>([]);
  const agentChatEventFrame = useRef<number | undefined>(undefined);
  const drainingQueuedPrompts = useRef(new Set<string>());
  const sideChatInstance = useRef(0);
  const sideChatAgentId = useRef<string | undefined>(undefined);
  const sideChatAgentIds = useRef(new Set<string>());

  const sidebarCollapsed = resolveSidebarCollapsed(
    mobileLayout,
    desktopSidebarCollapsed,
    mobileSidebarOpen,
  );

  const closeMobileNavigation = useCallback((): void => {
    if (!mobileLayout) return;
    setMobileSidebarOpen(false);
    setProfileOpen(false);
    window.requestAnimationFrame(() => mobileMenuButtonRef.current?.focus());
  }, [mobileLayout]);

  const openSidebar = useCallback((): void => {
    setProfileOpen(false);
    if (mobileLayout) setMobileSidebarOpen(true);
    else setDesktopSidebarCollapsed(false);
  }, [mobileLayout]);

  const toggleSidebar = useCallback((): void => {
    setProfileOpen(false);
    if (mobileLayout) {
      if (mobileSidebarOpen) closeMobileNavigation();
      else setMobileSidebarOpen(true);
    } else {
      setDesktopSidebarCollapsed((collapsed) => !collapsed);
    }
  }, [closeMobileNavigation, mobileLayout, mobileSidebarOpen]);

  useEffect(() => {
    if (desktopRuntime || typeof window.matchMedia !== "function") return;
    const query = window.matchMedia(MOBILE_LAYOUT_QUERY);
    const sync = (): void => setMobileQueryMatches(query.matches);
    sync();
    query.addEventListener("change", sync);
    return () => query.removeEventListener("change", sync);
  }, [desktopRuntime]);

  useEffect(() => {
    setMobileSidebarOpen(false);
    setProfileOpen(false);
  }, [mobileLayout]);

  useEffect(() => {
    if (!mobileLayout) {
      setMobileViewportHeight(undefined);
      return;
    }
    const viewport = window.visualViewport;
    const sync = (): void => {
      setMobileViewportHeight(
        Math.round(viewport?.height ?? window.innerHeight),
      );
    };
    sync();
    window.addEventListener("resize", sync);
    viewport?.addEventListener("resize", sync);
    return () => {
      window.removeEventListener("resize", sync);
      viewport?.removeEventListener("resize", sync);
    };
  }, [mobileLayout]);

  useEffect(() => {
    if (!mobileLayout || !mobileSidebarOpen) return;
    const closeOnEscape = (event: globalThis.KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      closeMobileNavigation();
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [closeMobileNavigation, mobileLayout, mobileSidebarOpen]);

  const { project: activeProject, conversation: activeConversation } = useMemo(
    () => getActive(desktop),
    [desktop],
  );
  const defaultModel = models.find((model) => model.isDefault) ?? models[0];
  const selectedModel =
    models.find(
      (model) =>
        model.id === activeConversation?.modelId ||
        model.model === activeConversation?.modelId,
    ) ?? defaultModel;
  const supportedThinkingLevels = thinkingLevelsForModel(selectedModel);
  const effort = normalizeThinkingLevel(
    activeConversation?.thinkingLevel,
    selectedModel,
  );
  const permissionMode = activeConversation?.permissionMode ?? "manual";
  const activeTurn = activeConversation
    ? inFlightTurns[activeConversation.id]
    : undefined;
  const activeQueuedPrompts = activeConversation
    ? queuedPrompts[activeConversation.id] ?? []
    : [];
  const activeRemoteQueuedPrompts = activeConversation
    ? remoteQueuedPrompts[activeConversation.id] ?? []
    : [];
  const activeSubagentRuns = activeConversation
    ? subagentRuns[activeConversation.id]
    : undefined;
  const activeSubagentLiveTurns = activeConversation
    ? subagentLiveTurns[activeConversation.id]
    : undefined;
  const activeHistory = activeConversation
    ? historyByConversation[activeConversation.id]
    : undefined;

  useEffect(() => {
    inFlightTurnsRef.current = inFlightTurns;
  }, [inFlightTurns]);
  const visibleHistoryMessages = useMemo(
    () =>
      (activeHistory
        ? activeTurn
          ? historyBeforeInFlightTurn(activeHistory.items, activeTurn)
          : activeHistory.items
        : []
      ).filter(isVisibleHistoryMessage),
    [
      activeHistory?.items,
      activeTurn?.historyBoundaryId,
      activeTurn?.userMessageId,
      activeTurn?.prompt,
    ],
  );
  const historyToolPresentation = useMemo(
    () => mergeHistoryToolResults(visibleHistoryMessages),
    [visibleHistoryMessages],
  );
  const latestHistoryCompactionSummaryId = [...visibleHistoryMessages]
    .reverse()
    .find(
      (message) => messageOriginKind(message) === "compaction_summary",
    )?.id;
  const historyConversationTurns = useMemo(
    () => groupHistoryMessages(historyToolPresentation.messages),
    [historyToolPresentation.messages],
  );
  const liveOutlineTurnId = activeTurn
    ? `live-${activeTurn.turnId ?? activeTurn.createdAt}`
    : undefined;
  const conversationOutlineItems = useMemo<ConversationOutlineItem[]>(() => {
    const items = historyConversationTurns.flatMap((turn) => {
      if (!turn.user) return [];
      const finalResponse = finalResponseMessage(turn.responses);
      const responseText = finalResponse ? messageText(finalResponse) : "";
      const messageLength = turn.responses.reduce(
        (total, message) => total + messageText(message).length,
        0,
      );
      return [
        {
          id: turn.id,
          title:
            compactOutlineText(messageText(turn.user), 120) || t("message.user"),
          previewLines: conversationOutlinePreview(responseText),
          tickWidth: outlineTickWidth(messageLength),
        },
      ];
    });

    if (activeTurn && liveOutlineTurnId) {
      const responseText = activeTurn.steps
        .flatMap((step) =>
          step.blocks.flatMap((block) =>
            block.kind === "text" ? [block.content] : [],
          ),
        )
        .join("\n");
      items.push({
        id: liveOutlineTurnId,
        title: compactOutlineText(activeTurn.prompt, 120) || t("message.user"),
        previewLines: conversationOutlinePreview(responseText),
        tickWidth: outlineTickWidth(responseText.length),
      });
    }

    return items;
  }, [activeTurn, historyConversationTurns, liveOutlineTurnId]);
  const hasVisibleMessages =
    historyToolPresentation.messages.length > 0 ||
    activeTurn !== undefined ||
    activeQueuedPrompts.length > 0 ||
    activeRemoteQueuedPrompts.length > 0;
  const isStreaming = isTurnRunning(activeTurn);
  const composerHasContent =
    prompt.trim().length > 0 ||
    promptAttachments.length > 0 ||
    promptSkills.length > 0;
  const showStopButton = isStreaming && !composerHasContent;
  const isHistoryLoading =
    activeConversation !== undefined &&
    (activeHistory === undefined || activeHistory.loading);
  const activeApproval = activeConversation
    ? interactions[activeConversation.id]?.find(
        (interaction) => interaction.kind === "approval",
      )
    : undefined;
  const activeQuestion = activeConversation
    ? interactions[activeConversation.id]?.find(
        (interaction) => interaction.kind === "question",
      )
    : undefined;
  const hasBlockingInteraction =
    activeApproval !== undefined || activeQuestion !== undefined;
  const activeCompaction = activeConversation
    ? compactions[activeConversation.id]
    : undefined;
  const activeContextUsage = activeConversation
    ? contextUsages[activeConversation.id]
    : undefined;
  const activeContextPercent = activeContextUsage
    ? Math.round(
        Math.max(
          0,
          Math.min(
            1,
            activeContextUsage.usageRatio ||
              (activeContextUsage.maxContextTokens > 0
                ? activeContextUsage.contextTokens /
                  activeContextUsage.maxContextTokens
                : 0),
          ),
        ) * 100,
      )
    : undefined;
  const canRunCompaction =
    activeAgentScope !== undefined &&
    !isStreaming &&
    activeCompaction?.phase !== "started" &&
    !compactionCommandBusy &&
    !forkCommandBusy;
  const canRunFork =
    activeProject !== undefined &&
    activeConversation !== undefined &&
    activeAgentScope?.sessionId === activeConversation.id &&
    !isStreaming &&
    activeCompaction?.phase !== "started" &&
    !compactionCommandBusy &&
    !forkCommandBusy;
  const canOpenSideChat =
    activeConversation !== undefined &&
    activeAgentScope?.sessionId === activeConversation.id &&
    activeCompaction?.phase !== "started";
  const activeAgentUsage = activeConversation
    ? agentUsages[activeConversation.id]
    : undefined;
  const activePlan = activeConversation
    ? plans[activeConversation.id]
    : undefined;
  const activeTodos = activeConversation
    ? (sessionTodos[activeConversation.id] ?? [])
    : [];
  const activeBackgroundTasks = activeConversation
    ? (backgroundTasks[activeConversation.id] ?? []).filter(
        (task) => task.kind === "process" && task.detached !== false,
      )
    : [];
  const activeRunningTaskKey = activeBackgroundTasks
    .filter((task) => task.status === "running")
    .map((task) => task.taskId)
    .join("\u0000");

  const updateDesktop = (
    recipe: (current: DesktopState) => DesktopState,
  ): void => {
    setDesktop((current) => recipe(current));
  };

  const showNotice = (message: string): void => {
    setNotice(message);
    if (noticeTimer.current) window.clearTimeout(noticeTimer.current);
    noticeTimer.current = window.setTimeout(() => setNotice(undefined), 3600);
  };

  const closeSideChat = useCallback((): void => {
    sideChatInstance.current += 1;
    sideChatAgentId.current = undefined;
    setSideChat(undefined);
  }, []);

  const loadBackgroundTaskOutput = useCallback(
    async (
      scope: { sessionId: string; agentId: string },
      taskId: string,
      tail = BACKGROUND_TASK_OUTPUT_TAIL,
    ): Promise<void> => {
      setBackgroundTasks((current) => ({
        ...current,
        [scope.sessionId]: (current[scope.sessionId] ?? []).map((task) =>
          task.taskId === taskId
            ? { ...task, outputLoading: true, outputError: undefined }
            : task,
        ),
      }));
      try {
        const output = await createAgentClient(scope).getTaskOutput(taskId, tail);
        setBackgroundTasks((current) => ({
          ...current,
          [scope.sessionId]: (current[scope.sessionId] ?? []).map((task) =>
            task.taskId === taskId
              ? {
                  ...task,
                  output,
                  outputLoading: false,
                  outputError: undefined,
                }
              : task,
          ),
        }));
      } catch (error) {
        setBackgroundTasks((current) => ({
          ...current,
          [scope.sessionId]: (current[scope.sessionId] ?? []).map((task) =>
            task.taskId === taskId
              ? {
                  ...task,
                  outputLoading: false,
                  outputError: conciseError(error),
                }
              : task,
          ),
        }));
      }
    },
    [],
  );

  const refreshBackgroundTasks = useCallback(
    async (scope: { sessionId: string; agentId: string }): Promise<void> => {
      const request = (backgroundTaskRequests.current[scope.sessionId] ?? 0) + 1;
      backgroundTaskRequests.current[scope.sessionId] = request;
      const tasks = await createAgentClient(scope).getTasks({
        activeOnly: false,
        limit: BACKGROUND_TASK_LIST_LIMIT,
      });
      if (request !== backgroundTaskRequests.current[scope.sessionId]) return;

      const sortedTasks = [...tasks].sort(
        (left, right) => right.startedAt - left.startedAt,
      );
      setBackgroundTasks((current) => {
        const previous = new Map(
          (current[scope.sessionId] ?? []).map((task) => [task.taskId, task]),
        );
        return {
          ...current,
          [scope.sessionId]: sortedTasks.map((task) => {
            const cached = previous.get(task.taskId);
            return {
              ...task,
              output: cached?.output,
              outputLoading: cached?.outputLoading,
              outputError: cached?.outputError,
            };
          }),
        };
      });

      const visibleTasks = sortedTasks.filter(
        (task) =>
          task.kind === "process" &&
          task.detached !== false &&
          task.status === "running",
      );
      void Promise.all(
        visibleTasks.map((task) =>
          loadBackgroundTaskOutput(
            scope,
            task.taskId,
            BACKGROUND_TASK_OUTPUT_TAIL,
          ),
        ),
      );
    },
    [loadBackgroundTaskOutput],
  );

  const refreshAgentState = async (scope: {
    sessionId: string;
    agentId: string;
  }): Promise<void> => {
    const agent = createAgentClient(scope);
    const [plan, todos, usage, permission] = await Promise.all([
      agent.getPlan(),
      agent.getTodos(),
      agent.getUsage(),
      agent.getPermission(),
    ]);
    setPlans((current) => ({ ...current, [scope.sessionId]: plan }));
    setSessionTodos((current) => ({
      ...current,
      [scope.sessionId]: todos,
    }));
    setAgentUsages((current) => ({
      ...current,
      [scope.sessionId]: usage,
    }));
    updateDesktop((current) => ({
      ...current,
      projects: current.projects.map((project) => ({
        ...project,
        conversations: project.conversations.map((conversation) =>
          conversation.id === scope.sessionId
            ? { ...conversation, permissionMode: permission.mode }
            : conversation,
        ),
      })),
    }));
  };

  const releaseAgentSubscription = (sessionId: string): void => {
    const subscription = agentSubscriptions.current.get(sessionId);
    agentSubscriptions.current.delete(sessionId);
    pendingAgentSubscriptions.current.delete(sessionId);
    if (subscription) {
      void unsubscribeAgentEvents(subscription.subscriptionId);
    }
  };

  const releaseAllAgentSubscriptions = (): void => {
    for (const subscription of agentSubscriptions.current.values()) {
      void unsubscribeAgentEvents(subscription.subscriptionId);
    }
    agentSubscriptions.current.clear();
    pendingAgentSubscriptions.current.clear();
  };

  const ensureAgentSubscription = async (scope: {
    sessionId: string;
    agentId: string;
  }): Promise<void> => {
    const existing = agentSubscriptions.current.get(scope.sessionId);
    if (existing?.agentId === scope.agentId) return;
    if (existing) releaseAgentSubscription(scope.sessionId);

    const pending = pendingAgentSubscriptions.current.get(scope.sessionId);
    if (pending?.agentId === scope.agentId) {
      await pending.promise;
      if (
        agentSubscriptions.current.get(scope.sessionId)?.agentId ===
        scope.agentId
      ) {
        return;
      }
      return ensureAgentSubscription(scope);
    }
    if (pending) pendingAgentSubscriptions.current.delete(scope.sessionId);

    const promise = subscribeAgentEvents(scope);
    pendingAgentSubscriptions.current.set(scope.sessionId, {
      agentId: scope.agentId,
      promise,
    });

    let subscriptionId: string;
    try {
      subscriptionId = await promise;
    } catch (error) {
      if (
        pendingAgentSubscriptions.current.get(scope.sessionId)?.promise ===
        promise
      ) {
        pendingAgentSubscriptions.current.delete(scope.sessionId);
      }
      throw error;
    }
    const current = pendingAgentSubscriptions.current.get(scope.sessionId);
    if (current?.promise !== promise) {
      await unsubscribeAgentEvents(subscriptionId);
      return;
    }
    pendingAgentSubscriptions.current.delete(scope.sessionId);
    agentSubscriptions.current.set(scope.sessionId, {
      agentId: scope.agentId,
      subscriptionId,
    });
  };

  const loadModels = async (): Promise<void> => {
    setModelsBusy(true);
    try {
      const nextModels = await invoke<Model[]>("list_models");
      setModels(nextModels);
      if (nextModels.length === 0) showNotice(t("notice.noModelsConfigured"));
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setModelsBusy(false);
    }
  };

  const refreshModels = async (): Promise<void> => {
    try {
      const nextModels = await invoke<Model[]>("refresh_models");
      setModels(nextModels);
      if (nextModels.length === 0) showNotice(t("notice.noModelsForAccount"));
    } catch {
      // Keep using the configured model list when the background refresh fails.
    }
  };

  const loadAccountUsage = async (): Promise<void> => {
    const request = accountUsageRequest.current + 1;
    accountUsageRequest.current = request;
    setAccountUsageBusy(true);
    setAccountUsageError(undefined);
    try {
      const usage = await invoke<AccountUsage>("account_usage");
      if (request === accountUsageRequest.current) setAccountUsage(usage);
    } catch (error) {
      if (request === accountUsageRequest.current) {
        setAccountUsageError(conciseError(error));
      }
    } finally {
      if (request === accountUsageRequest.current) setAccountUsageBusy(false);
    }
  };

  const toggleProfile = (): void => {
    const opening = !profileOpen;
    setProfileOpen(opening);
    if (opening && auth.loggedIn) void loadAccountUsage();
  };

  const openSettings = (): void => {
    setProfileOpen(false);
    setSettingsOpen(true);
  };

  const closeSettings = useCallback((): void => {
    setSettingsOpen(false);
  }, []);

  const updateColorScheme = (nextColorScheme: ColorScheme): void => {
    setColorScheme(nextColorScheme);
    saveColorScheme(nextColorScheme);
  };

  const updateLanguage = (nextLanguage: Language): void => {
    setLanguage(nextLanguage);
    setLanguageState(nextLanguage);
    saveLanguage(nextLanguage);
  };

  useLayoutEffect(() => {
    applyColorScheme(colorScheme);
  }, [colorScheme]);

  useLayoutEffect(() => {
    setLanguage(language);
    applyLanguage(language);
  }, [language]);

  useEffect(() => {
    let active = true;
    const request = desktopInventoryRequest.current + 1;
    desktopInventoryRequest.current = request;
    loadDesktopState()
      .then((state) => {
        if (active && request === desktopInventoryRequest.current) {
          setDesktop((current) => mergeDesktopInventory(current, state));
        }
      })
      .catch(() => {
        // Vite's browser preview has no Tauri bridge.
      });
    void loadModels().then(() => {
      if (!active) return;
      void invoke<AuthStatus>("auth_status")
        .then((status) => {
          if (!active) return;
          setAuth(status);
          if (status.loggedIn) void refreshModels();
        })
        .catch(() => {
          // Vite's browser preview has no Tauri bridge; the actual desktop app does.
        });
    });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    void getAppVersion()
      .then((version) => {
        if (active) setAppVersion(version);
      })
      .catch(() => undefined);
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    if (!profileOpen) return;
    const closeProfile = (event: PointerEvent): void => {
      if (
        event.target instanceof Node &&
        !profileRef.current?.contains(event.target)
      ) {
        setProfileOpen(false);
      }
    };
    const closeProfileOnEscape = (event: globalThis.KeyboardEvent): void => {
      if (event.key === "Escape") setProfileOpen(false);
    };
    document.addEventListener("pointerdown", closeProfile);
    document.addEventListener("keydown", closeProfileOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeProfile);
      document.removeEventListener("keydown", closeProfileOnEscape);
    };
  }, [profileOpen]);

  useEffect(() => {
    if (!composerAddOpen) return;
    const closeComposerAdd = (event: PointerEvent): void => {
      if (
        event.target instanceof Node &&
        !composerAddRef.current?.contains(event.target)
      ) {
        setComposerAddOpen(false);
      }
    };
    const closeComposerAddOnEscape = (
      event: globalThis.KeyboardEvent,
    ): void => {
      if (event.key === "Escape") setComposerAddOpen(false);
    };
    document.addEventListener("pointerdown", closeComposerAdd);
    document.addEventListener("keydown", closeComposerAddOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeComposerAdd);
      document.removeEventListener("keydown", closeComposerAddOnEscape);
    };
  }, [composerAddOpen]);

  useEffect(() => {
    if (slashMenuOpen) setSlashMenuActiveIndex(0);
  }, [slashMenuOpen]);

  useEffect(() => {
    skillsRequest.current += 1;
    skillDetailRequest.current += 1;
    setComposerAddOpen(false);
    setAvailableSkills([]);
    setSkillsBusy(false);
    setSkillsError(undefined);
    setPromptSkills([]);
    setSlashMenuOpen(false);
    closeSideChat();
    setCompactionSummaryDetail(undefined);
    setSkillDetailTarget(undefined);
    setSkillDetail(undefined);
    setSkillDetailBusy(false);
    setSkillDetailError(undefined);
  }, [activeConversation?.id, closeSideChat]);

  useEffect(() => {
    setActiveAgentScope(undefined);
    if (!activeProject || !activeConversation || !selectedModel) {
      return;
    }
    let disposed = false;
    void prepareSession({
      sessionId: activeConversation.id,
      workDir: activeProject.path,
    })
      .then(async (scope) => {
        await ensureAgentSubscription(scope);
        if (disposed) return;
        const sessionModel = models.find(
          (model) => model.id === scope.model || model.model === scope.model,
        );
        const thinkingLevel = normalizeThinkingLevel(
          scope.thinkingLevel,
          sessionModel,
        );
        if (
          sessionModel?.supportsReasoning &&
          thinkingLevel !== scope.thinkingLevel
        ) {
          await createAgentClient(scope).setThinking(thinkingLevel);
        }
        updateDesktop((current) => ({
          ...current,
          projects: current.projects.map((project) =>
            project.id !== activeProject.id
              ? project
              : {
                  ...project,
                  conversations: project.conversations.map((conversation) =>
                    conversation.id === activeConversation.id
                      ? {
                          ...conversation,
                          modelId: scope.model,
                          thinkingLevel,
                          permissionMode: scope.permissionMode,
                        }
                      : conversation,
                  ),
                },
          ),
        }));
        setActiveAgentScope(scope);
        await refreshAgentState(scope);
        void refreshBackgroundTasks(scope).catch(() => {
          // Sessions without task state simply have no background task pill.
        });
      })
      .catch((error) => {
        if (!disposed) showNotice(conciseError(error));
      });
    return () => {
      disposed = true;
      setActiveAgentScope(undefined);
    };
  }, [
    activeConversation?.id,
    activeProject?.path,
    models.length,
    refreshBackgroundTasks,
  ]);

  useEffect(() => {
    if (!activeAgentScope || !activeRunningTaskKey) return;
    const timer = window.setInterval(() => {
      void refreshBackgroundTasks(activeAgentScope).catch(() => {
        // The lifecycle event or next poll will retry a transient failure.
      });
    }, 1000);
    return () => window.clearInterval(timer);
  }, [
    activeAgentScope?.agentId,
    activeAgentScope?.sessionId,
    activeRunningTaskKey,
    refreshBackgroundTasks,
  ]);

  useEffect(
    () => () => releaseAllAgentSubscriptions(),
    [],
  );

  useEffect(() => {
    const refreshDesktopInventory = async (): Promise<void> => {
      const request = desktopInventoryRequest.current + 1;
      desktopInventoryRequest.current = request;
      try {
        const inventory = await loadDesktopState();
        if (request !== desktopInventoryRequest.current) return;
        setDesktop((current) => mergeDesktopInventory(current, inventory));
      } catch {
        // A later state-change event or explicit action will retry the refresh.
      }
    };
    const unlistenDevice = listen<DeviceCode>("auth-device-code", (event) => {
      setDeviceCode(event.payload);
      setLoginOpen(true);
    });
    const unlistenAuthRequired = listen(TRANSPORT_AUTH_REQUIRED, () => {
      setWebAuthOpen(true);
    });
    const unlistenReplayReset = listen<ReplayResetEvent>(
      TRANSPORT_REPLAY_RESET,
      async (event) => {
        await refreshDesktopInventory();
        const scopes = new Map(
          event.payload.scopes.map((scope) => [scope.sessionId, scope]),
        );
        await Promise.all(
          [...scopes.values()].map(async (scope) => {
            try {
              const permission = await createAgentClient(scope).getPermission();
              updateDesktop((current) => ({
                ...current,
                projects: current.projects.map((project) => ({
                  ...project,
                  conversations: project.conversations.map((conversation) =>
                    conversation.id === scope.sessionId
                      ? { ...conversation, permissionMode: permission.mode }
                      : conversation,
                  ),
                })),
              }));
            } catch {
              // Session preparation or a later status event will retry it.
            }
          }),
        );
        const sessionIds = new Set(
          event.payload.scopes.map((scope) => scope.sessionId),
        );
        if (sessionIds.size === 0) return;
        if (agentChatEventFrame.current !== undefined) {
          window.cancelAnimationFrame(agentChatEventFrame.current);
          agentChatEventFrame.current = undefined;
        }
        queuedAgentChatEvents.current = queuedAgentChatEvents.current.filter(
          (queued) => !sessionIds.has(queued.sessionId),
        );
        setInFlightTurns((current) => {
          const next = omitSessionKeys(current, sessionIds);
          inFlightTurnsRef.current = next;
          return next;
        });
        setSubagentLiveTurns((current) => omitSessionKeys(current, sessionIds));
        setSubagentRuns((current) => omitSessionKeys(current, sessionIds));
        setInteractions((current) => omitSessionKeys(current, sessionIds));
        setRemoteQueuedPrompts((current) =>
          omitSessionKeys(current, sessionIds),
        );

        await Promise.all(
          [...sessionIds].map(async (conversationId) => {
            const request =
              (historyRequests.current[conversationId] ?? 0) + 1;
            historyRequests.current[conversationId] = request;
            try {
              const page = await fetchConversationHistory(conversationId);
              if (request !== historyRequests.current[conversationId]) return;
              setHistoryByConversation((current) => ({
                ...current,
                [conversationId]: {
                  conversationId,
                  items: [...page.items].reverse(),
                  loading: false,
                },
              }));
            } catch (error) {
              if (request !== historyRequests.current[conversationId]) return;
              setHistoryByConversation((current) => ({
                ...current,
                [conversationId]: {
                  conversationId,
                  items: current[conversationId]?.items ?? [],
                  loading: false,
                  error: conciseError(error),
                },
              }));
            }
          }),
        );
      },
    );
    const unlistenDesktopStateChanged = listen(
      "desktop-state-changed",
      refreshDesktopInventory,
    );
    const unlistenBrowserError = listen<string>(
      "auth-browser-open-failed",
      (event) => {
        showNotice(t("notice.browserOpenFailed", { error: event.payload }));
      },
    );
    const unlistenChatEvent = listen<AgentChatEventEnvelope>(
      "agent-event",
      (event) => {
        const payload = event.payload;
        const isMainAgentEvent = payload.agentId === MAIN_AGENT_ID;
        const isSideChatEvent =
          payload.agentId === sideChatAgentId.current;
        const isSideChatAgent =
          sideChatAgentIds.current.has(payload.agentId);
        const submitted = readPromptSubmittedEvent(payload.event);
        if (submitted && isMainAgentEvent && !isSideChatAgent) {
          const projected = inFlightTurnFromUserMessage(submitted);
          const existing = inFlightTurnsRef.current[payload.sessionId];
          if (
            !existing ||
            isSameLiveUserMessage(existing, submitted) ||
            (existing.status === "queued" && !existing.promptId)
          ) {
            setInFlightTurns((current) => {
              const active = current[payload.sessionId];
              if (
                active &&
                !isSameLiveUserMessage(active, submitted) &&
                !(active.status === "queued" && !active.promptId)
              ) {
                return current;
              }
              const merged = {
                ...projected,
                ...active,
                promptId: submitted.promptId,
                userMessageId: submitted.userMessageId,
                prompt: projected.prompt,
                attachments: projected.attachments,
                skills: projected.skills,
                createdAt: projected.createdAt,
              };
              const next = { ...current, [payload.sessionId]: merged };
              inFlightTurnsRef.current = next;
              return next;
            });
          } else {
            setRemoteQueuedPrompts((current) => {
              const queued = current[payload.sessionId] ?? [];
              if (queued.some((item) => item.promptId === submitted.promptId)) {
                return current;
              }
              return {
                ...current,
                [payload.sessionId]: [
                  ...queued,
                  {
                    promptId: submitted.promptId,
                    userMessageId: submitted.userMessageId,
                    text: projected.prompt,
                    attachments: projected.attachments,
                    skills: projected.skills,
                    createdAt: projected.createdAt,
                  },
                ],
              };
            });
          }
        }
        if (
          isMainAgentEvent &&
          (payload.event.type === "prompt.completed" ||
            payload.event.type === "prompt.aborted") &&
          typeof payload.event.promptId === "string"
        ) {
          const promptId = payload.event.promptId;
          setRemoteQueuedPrompts((current) => ({
            ...current,
            [payload.sessionId]: (current[payload.sessionId] ?? []).filter(
              (item) => item.promptId !== promptId,
            ),
          }));
        }
        if (isAgentChatEvent(payload.event)) {
          const chatEvent = payload.event;
          if (
            isMainAgentEvent &&
            chatEvent.type === "turn.started" &&
            chatEvent.userMessage
          ) {
            const promptId = chatEvent.userMessage.promptId;
            setRemoteQueuedPrompts((current) => ({
              ...current,
              [payload.sessionId]: (current[payload.sessionId] ?? []).filter(
                (item) => item.promptId !== promptId,
              ),
            }));
          }
          if (isSideChatEvent) {
            setSideChat((current) => {
              if (
                !current ||
                current.parentSessionId !== payload.sessionId
              ) {
                return current;
              }
              const turns = [...current.turns];
              const last = turns.at(-1);
              if (!last) return current;
              turns[turns.length - 1] = reduceAgentChatEvent(
                last,
                chatEvent,
              );
              return { ...current, turns, starting: false };
            });
          } else if (!isSideChatAgent) {
            queuedAgentChatEvents.current.push({
              sessionId: payload.sessionId,
              agentId: payload.agentId,
              event: chatEvent,
            });
            if (agentChatEventFrame.current === undefined) {
              agentChatEventFrame.current = window.requestAnimationFrame(() => {
                agentChatEventFrame.current = undefined;
                const queue = queuedAgentChatEvents.current;
                queuedAgentChatEvents.current = [];
                if (queue.length > 0) {
                  const mainEvents = queue.filter(
                    (queued) => queued.agentId === MAIN_AGENT_ID,
                  );
                  const subagentEvents = queue.filter(
                    (queued) => queued.agentId !== MAIN_AGENT_ID,
                  );
                  if (mainEvents.length > 0) {
                    setInFlightTurns((current) =>
                      reduceQueuedAgentChatEvents(current, mainEvents),
                    );
                  }
                  if (subagentEvents.length > 0) {
                    setSubagentLiveTurns((current) =>
                      reduceQueuedSubagentChatEvents(current, subagentEvents),
                    );
                  }
                }
              });
            }
          }
        }
        if (!isSideChatAgent && isSubagentEvent(payload.event)) {
          const subagentEvent = payload.event;
          setSubagentRuns((current) =>
            mergeSessionSubagentEvent(
              current,
              payload.sessionId,
              subagentEvent,
            ),
          );
        }
        if (
          isMainAgentEvent &&
          isTaskLifecycleEventType(payload.event.type)
        ) {
          const started =
            payload.event.type === "task.started" ||
            payload.event.type === "background.task.started";
          const info = readAgentTaskInfo(
            payload.event.info,
            started ? "running" : undefined,
          );
          if (info) {
            setBackgroundTasks((current) => {
              const tasks = current[payload.sessionId] ?? [];
              const previous = tasks.find(
                (task) => task.taskId === info.taskId,
              );
              const nextTask: BackgroundTaskView = {
                ...previous,
                ...info,
              };
              const nextTasks = [
                nextTask,
                ...tasks.filter((task) => task.taskId !== info.taskId),
              ].sort((left, right) => right.startedAt - left.startedAt);
              return {
                ...current,
                [payload.sessionId]: nextTasks,
              };
            });
          }
          const taskScope = {
            sessionId: payload.sessionId,
            agentId: payload.agentId,
          };
          void refreshBackgroundTasks(taskScope).catch(() => {
            // The event payload already supplied the lifecycle update.
          });
          if (
            info?.kind === "process" &&
            info.detached !== false &&
            !started
          ) {
            void loadBackgroundTaskOutput(
              taskScope,
              info.taskId,
              BACKGROUND_TASK_DETAIL_TAIL,
            );
          }
        }
        if (
          isMainAgentEvent &&
          payload.event.type.startsWith("compaction.")
        ) {
          const phase = payload.event.type.slice("compaction.".length);
          if (
            phase === "started" ||
            phase === "completed" ||
            phase === "cancelled"
          ) {
            if (phase === "started") {
              setCompactionHistoryReady((current) => ({
                ...current,
                [payload.sessionId]: false,
              }));
            }
            const result =
              payload.event.result &&
              typeof payload.event.result === "object"
                ? (payload.event.result as Record<string, unknown>)
                : undefined;
            setCompactions((current) => ({
              ...current,
              [payload.sessionId]: {
                phase,
                trigger:
                  payload.event.trigger === "manual" ||
                  payload.event.trigger === "auto"
                    ? payload.event.trigger
                    : undefined,
                compactedCount:
                  typeof result?.compactedCount === "number"
                    ? result.compactedCount
                    : undefined,
                tokensBefore:
                  typeof result?.tokensBefore === "number"
                    ? result.tokensBefore
                    : undefined,
                tokensAfter:
                  typeof result?.tokensAfter === "number"
                    ? result.tokensAfter
                    : undefined,
              },
            }));
          }
        }
        if (isMainAgentEvent && payload.event.type === "todo.updated") {
          const todos = readTodoItems(payload.event.todos);
          if (todos) {
            setSessionTodos((current) => ({
              ...current,
              [payload.sessionId]: todos,
            }));
          }
        }
        if (
          payload.event.type === "agent.status.updated" &&
          isMainAgentEvent &&
          typeof payload.event.planMode === "boolean"
        ) {
          void createAgentClient({
            sessionId: payload.sessionId,
            agentId: payload.agentId,
          })
            .getPlan()
            .then((plan) => {
              setPlans((current) => ({
                ...current,
                [payload.sessionId]: plan,
              }));
            })
            .catch((error) => showNotice(conciseError(error)));
        }
        if (payload.event.type === "agent.status.updated" && isMainAgentEvent) {
          const model =
            typeof payload.event.model === "string"
              ? payload.event.model
              : undefined;
          const thinkingLevel =
            typeof payload.event.thinkingEffort === "string"
              ? payload.event.thinkingEffort
              : undefined;
          const permission = ["manual", "auto", "yolo"].includes(
            String(payload.event.permission),
          )
            ? (payload.event.permission as PermissionMode)
            : undefined;
          if (model || thinkingLevel || permission) {
            updateDesktop((current) => ({
              ...current,
              projects: current.projects.map((project) => ({
                ...project,
                conversations: project.conversations.map((conversation) =>
                  conversation.id === payload.sessionId
                    ? {
                        ...conversation,
                        ...(model ? { modelId: model } : {}),
                        ...(thinkingLevel ? { thinkingLevel } : {}),
                        ...(permission ? { permissionMode: permission } : {}),
                      }
                    : conversation,
                ),
              })),
            }));
          }
        }
        if (
          payload.event.type === "session.meta.updated" &&
          isMainAgentEvent &&
          typeof payload.event.title === "string"
        ) {
          const title = payload.event.title;
          updateDesktop((current) => ({
            ...current,
            projects: current.projects.map((project) => ({
              ...project,
              conversations: project.conversations.map((conversation) =>
                conversation.id === payload.sessionId
                  ? { ...conversation, title }
                  : conversation,
              ),
            })),
          }));
        }
        if (
          payload.event.type === "agent.status.updated" &&
          isMainAgentEvent &&
          payload.event.usage &&
          typeof payload.event.usage === "object"
        ) {
          setAgentUsages((current) => ({
            ...current,
            [payload.sessionId]: payload.event.usage as AgentUsageStatus,
          }));
        }
        if (
          isMainAgentEvent &&
          (payload.event.type === "agent.status.updated" ||
            payload.event.type === "context.spliced")
        ) {
          void invoke<ContextUsage | null>("conversation_context_usage", {
            sessionId: payload.sessionId,
          }).then((usage) => {
            if (!usage) return;
            setContextUsages((current) => ({
              ...current,
              [payload.sessionId]: usage,
            }));
          });
        }
      },
    );
    const unlistenInteractions = listen<AgentInteractionsEvent>(
      "agent-interactions",
      (event) => {
        setInteractions((current) => ({
          ...current,
          [event.payload.sessionId]: event.payload.interactions,
        }));
      },
    );
    return () => {
      if (agentChatEventFrame.current !== undefined) {
        window.cancelAnimationFrame(agentChatEventFrame.current);
        agentChatEventFrame.current = undefined;
      }
      queuedAgentChatEvents.current = [];
      void unlistenDevice.then((unlisten) => unlisten());
      void unlistenAuthRequired.then((unlisten) => unlisten());
      void unlistenReplayReset.then((unlisten) => unlisten());
      void unlistenDesktopStateChanged.then((unlisten) => unlisten());
      void unlistenBrowserError.then((unlisten) => unlisten());
      void unlistenChatEvent.then((unlisten) => unlisten());
      void unlistenInteractions.then((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => {
    const conversationId = activeConversation?.id;
    if (!conversationId) return;

    const request = (historyRequests.current[conversationId] ?? 0) + 1;
    historyRequests.current[conversationId] = request;
    setHistoryByConversation((current) =>
      current[conversationId]
        ? current
        : {
            ...current,
            [conversationId]: {
              conversationId,
              items: [],
              loading: true,
            },
          },
    );
    void fetchConversationHistory(conversationId)
      .then((page) => {
        if (request !== historyRequests.current[conversationId]) {
          return;
        }
        setHistoryByConversation((current) => ({
          ...current,
          [conversationId]: {
            conversationId,
            items: [...page.items].reverse(),
            loading: false,
          },
        }));
        setInFlightTurns((current) => {
          const turn = current[conversationId];
          if (
            !turn ||
            turn.status === "queued" ||
            turn.status === "running"
          ) {
            return current;
          }
          const next = { ...current };
          delete next[conversationId];
          return next;
        });
      })
      .catch((error) => {
        if (request !== historyRequests.current[conversationId]) {
          return;
        }
        setHistoryByConversation((current) => ({
          ...current,
          [conversationId]: {
            conversationId,
            items: current[conversationId]?.items ?? [],
            loading: false,
            error: conciseError(error),
          },
        }));
      });
  }, [activeConversation?.id]);

  useEffect(() => {
    const conversationId = activeConversation?.id;
    if (!conversationId) return;
    let active = true;
    invoke<ContextUsage | null>("conversation_context_usage", {
      sessionId: conversationId,
    })
      .then((usage) => {
        if (!active || !usage) return;
        setContextUsages((current) => ({
          ...current,
          [conversationId]: usage,
        }));
      })
      .catch(() => {
        // A new conversation does not have an agent session until its first prompt.
      });
    return () => {
      active = false;
    };
  }, [activeConversation?.id]);

  const updateActiveOutlineTurn = useCallback((): void => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    const anchors = Array.from(
      scroll.querySelectorAll<HTMLElement>("[data-conversation-turn-id]"),
    );
    if (anchors.length === 0) {
      setActiveOutlineTurnId(undefined);
      return;
    }

    const distanceFromBottom =
      scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight;
    let nextId = anchors.at(-1)?.dataset.conversationTurnId;
    if (distanceFromBottom > 48) {
      const scrollRect = scroll.getBoundingClientRect();
      const viewportMiddle = scrollRect.top + scrollRect.height / 2;
      nextId = anchors[0]?.dataset.conversationTurnId;
      for (const anchor of anchors) {
        if (anchor.getBoundingClientRect().top > viewportMiddle) break;
        nextId = anchor.dataset.conversationTurnId;
      }
    }

    setActiveOutlineTurnId((current) =>
      current === nextId ? current : nextId,
    );
  }, []);

  const scheduleActiveOutlineTurnUpdate = useCallback((): void => {
    if (outlineScrollFrameRef.current !== undefined) return;
    outlineScrollFrameRef.current = window.requestAnimationFrame(() => {
      outlineScrollFrameRef.current = undefined;
      updateActiveOutlineTurn();
    });
  }, [updateActiveOutlineTurn]);

  useLayoutEffect(() => {
    followLatestMessageRef.current = true;
    const scroll = scrollRef.current;
    if (scroll) {
      scroll.scrollTop = scroll.scrollHeight;
      lastChatScrollTopRef.current = scroll.scrollTop;
      lastChatScrollHeightRef.current = scroll.scrollHeight;
    }
  }, [activeConversation?.id, activeHistory?.loading]);

  useLayoutEffect(() => {
    const scroll = scrollRef.current;
    const content = messageStackRef.current;
    if (!scroll || !content || activeHistory?.loading) return;

    const scheduleScrollToLatest = (): void => {
      if (
        !followLatestMessageRef.current ||
        chatScrollFrameRef.current !== undefined
      ) {
        return;
      }
      chatScrollFrameRef.current = window.requestAnimationFrame(() => {
        chatScrollFrameRef.current = undefined;
        if (!followLatestMessageRef.current) return;
        scroll.scrollTop = scroll.scrollHeight;
        lastChatScrollTopRef.current = scroll.scrollTop;
        lastChatScrollHeightRef.current = scroll.scrollHeight;
      });
    };

    const observer = new ResizeObserver(scheduleScrollToLatest);
    observer.observe(content);
    scheduleScrollToLatest();
    return () => {
      observer.disconnect();
      if (chatScrollFrameRef.current !== undefined) {
        window.cancelAnimationFrame(chatScrollFrameRef.current);
        chatScrollFrameRef.current = undefined;
      }
    };
  }, [
    activeConversation?.id,
    activeHistory?.loading,
    hasVisibleMessages,
  ]);

  useLayoutEffect(() => {
    updateActiveOutlineTurn();
  }, [
    activeConversation?.id,
    conversationOutlineItems,
    updateActiveOutlineTurn,
  ]);

  useEffect(
    () => () => {
      if (outlineScrollFrameRef.current !== undefined) {
        window.cancelAnimationFrame(outlineScrollFrameRef.current);
      }
    },
    [],
  );

  const handleChatScroll = (): void => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    scheduleActiveOutlineTurnUpdate();
    const scrollingUp =
      scroll.scrollTop < lastChatScrollTopRef.current - 1;
    const contentHeightChanged =
      Math.abs(scroll.scrollHeight - lastChatScrollHeightRef.current) > 1;
    lastChatScrollTopRef.current = scroll.scrollTop;
    lastChatScrollHeightRef.current = scroll.scrollHeight;
    if (scrollingUp) {
      followLatestMessageRef.current = false;
      return;
    }
    const distanceFromBottom =
      scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight;
    if (distanceFromBottom <= 48) {
      followLatestMessageRef.current = true;
    } else if (!contentHeightChanged) {
      followLatestMessageRef.current = false;
    }
  };

  const scrollToConversationTurn = (turnId: string): void => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    const target = Array.from(
      scroll.querySelectorAll<HTMLElement>("[data-conversation-turn-id]"),
    ).find((anchor) => anchor.dataset.conversationTurnId === turnId);
    if (!target) return;
    followLatestMessageRef.current = false;
    setActiveOutlineTurnId(turnId);
    target.scrollIntoView({ behavior: "smooth", block: "center" });
  };

  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    textarea.style.height = "0px";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 180)}px`;
  }, [prompt]);

  useEffect(() => {
    const conversationId = activeConversation?.id;
    if (conversationId === promptUndoConversationRef.current) return;
    promptUndoConversationRef.current = conversationId;
    promptCompositionRef.current = false;
    promptUndoHistoryRef.current = createPromptUndoHistory(prompt);
  }, [activeConversation?.id]);

  const resetPrompt = (value = ""): void => {
    promptCompositionRef.current = false;
    promptUndoHistoryRef.current = createPromptUndoHistory(value);
    setPrompt(value);
    setSlashMenuOpen(false);
  };

  const updatePrompt = (value: string, isComposing = false): void => {
    const history = recordPromptInput(promptUndoHistoryRef.current, value, {
      isComposing,
    });
    promptUndoHistoryRef.current = history;
    setPrompt(value);
  };

  const syncSlashMenu = (textarea: HTMLTextAreaElement): void => {
    const open =
      document.activeElement === textarea &&
      textarea.value.startsWith("/") &&
      textarea.selectionStart === 1 &&
      textarea.selectionEnd === 1;
    setSlashMenuOpen(open);
    if (open) setComposerAddOpen(false);
  };

  const undoPrompt = (): void => {
    const history = undoPromptEdit(promptUndoHistoryRef.current);
    if (history === promptUndoHistoryRef.current) return;
    promptUndoHistoryRef.current = history;
    setPrompt(history.current);
    window.requestAnimationFrame(() => {
      const textarea = textareaRef.current;
      if (!textarea) return;
      textarea.focus();
      textarea.setSelectionRange(history.current.length, history.current.length);
    });
  };

  const forgetSessionState = (sessionIds: string[]): void => {
    const ids = new Set(sessionIds);
    if (ids.size === 0) return;
    for (const sessionId of ids) {
      delete historyRequests.current[sessionId];
      delete backgroundTaskRequests.current[sessionId];
      releaseAgentSubscription(sessionId);
    }
    setInteractions((current) => omitSessionKeys(current, ids));
    setCompactions((current) => omitSessionKeys(current, ids));
    setCompactionHistoryReady((current) => omitSessionKeys(current, ids));
    setContextUsages((current) => omitSessionKeys(current, ids));
    setAgentUsages((current) => omitSessionKeys(current, ids));
    setMessageDurations((current) => omitSessionKeys(current, ids));
    setPlans((current) => omitSessionKeys(current, ids));
    setSessionTodos((current) => omitSessionKeys(current, ids));
    setBackgroundTasks((current) => omitSessionKeys(current, ids));
    setSubagentRuns((current) => omitSessionKeys(current, ids));
    setSubagentLiveTurns((current) => omitSessionKeys(current, ids));
    setQueuedPrompts((current) => omitSessionKeys(current, ids));
    setRemoteQueuedPrompts((current) => omitSessionKeys(current, ids));
    setInFlightTurns((current) => {
      const next = omitSessionKeys(current, ids);
      inFlightTurnsRef.current = next;
      return next;
    });
    setHistoryByConversation((current) => omitSessionKeys(current, ids));
    if (activeConversation && ids.has(activeConversation.id)) {
      resetPrompt();
      setPromptAttachments([]);
      setResolvingInteraction(undefined);
    }
  };

  const confirmRemoval = async (): Promise<void> => {
    const target = removalTarget;
    if (!target || removalBusy) return;
    setRemovalBusy(true);
    try {
      if (target.kind === "project") {
        await removeWorkspace(target.projectId);
        forgetSessionState(target.conversationIds);
        updateDesktop((current) => {
          const removedIndex = current.projects.findIndex(
            (project) => project.id === target.projectId,
          );
          if (removedIndex < 0) return current;
          const projects = current.projects.filter(
            (project) => project.id !== target.projectId,
          );
          if (current.activeProjectId !== target.projectId) {
            return { ...current, projects };
          }
          const fallback =
            projects[Math.min(removedIndex, projects.length - 1)];
          return {
            projects,
            activeProjectId: fallback?.id,
            activeConversationId: fallback?.conversations[0]?.id,
          };
        });
        showNotice(t("notice.projectRemoved", { name: target.name }));
      } else {
        await archiveSession(target.conversationId);
        forgetSessionState([target.conversationId]);
        updateDesktop((current) => {
          const project = current.projects.find(
            (item) => item.id === target.projectId,
          );
          if (!project) return current;
          const removedIndex = project.conversations.findIndex(
            (conversation) => conversation.id === target.conversationId,
          );
          if (removedIndex < 0) return current;
          const conversations = project.conversations.filter(
            (conversation) => conversation.id !== target.conversationId,
          );
          const projects = current.projects.map((item) =>
            item.id === target.projectId
              ? { ...item, conversations }
              : item,
          );
          if (current.activeConversationId !== target.conversationId) {
            return { ...current, projects };
          }
          const fallback =
            conversations[Math.min(removedIndex, conversations.length - 1)];
          return {
            ...current,
            projects,
            activeProjectId: target.projectId,
            activeConversationId: fallback?.id,
          };
        });
        showNotice(t("notice.conversationArchived", { title: target.title }));
      }
      setRemovalTarget(undefined);
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setRemovalBusy(false);
    }
  };

  const addProjectPath = async (selection: string): Promise<void> => {
    try {
      const workspace = await createOrTouchWorkspace(selection);
      const sessions = await listWorkspaceSessions(workspace.id);
      const project = projectFromWorkspace(
        workspace,
        desktop.projects.length,
        sessions,
      );
      updateDesktop((current) => {
        const existing = current.projects.find(
          (item) => item.id === workspace.id || item.path === selection,
        );
        if (existing) {
          return {
            ...current,
            activeProjectId: existing.id,
            activeConversationId: existing.conversations[0]?.id,
            projects: current.projects.map((item) =>
              item.id === existing.id ? { ...item, expanded: true } : item,
            ),
          };
        }
        return {
          projects: [...current.projects, project],
          activeProjectId: project.id,
          activeConversationId: undefined,
        };
      });
      if (mobileLayout) closeMobileNavigation();
      else setDesktopSidebarCollapsed(false);
    } catch (error) {
      showNotice(conciseError(error));
    }
  };

  const addProject = async (): Promise<void> => {
    if (!isDesktop()) {
      setDirectoryPickerOpen(true);
      return;
    }
    try {
      const selection = await pickNativeDirectory();
      if (selection) await addProjectPath(selection);
    } catch (error) {
      showNotice(conciseError(error));
    }
  };

  const createConversation = async (
    project: Project,
    event?: MouseEvent<HTMLButtonElement>,
  ): Promise<void> => {
    event?.stopPropagation();
    const model = selectedModel ?? models[0];
    if (!model) {
      showNotice(t("notice.modelRequired"));
      return;
    }
    try {
      const scope = await prepareSession({
        workDir: project.path,
        model: model.id,
        thinking: effort,
        permission: permissionMode,
      });
      const conversation = {
        ...conversationFromSession(scope.sessionId),
        modelId: scope.model,
        thinkingLevel: scope.thinkingLevel,
        permissionMode: scope.permissionMode,
      };
      updateDesktop((current) => ({
        ...current,
        activeProjectId: project.id,
        activeConversationId: conversation.id,
        projects: current.projects.map((item) =>
          item.id === project.id
            ? {
                ...item,
                expanded: true,
                conversations: [
                  conversation,
                  ...item.conversations.filter(
                    (candidate) => candidate.id !== conversation.id,
                  ),
                ],
              }
            : item,
        ),
      }));
      resetPrompt();
      setPromptAttachments([]);
      closeMobileNavigation();
    } catch (error) {
      showNotice(conciseError(error));
    }
  };

  const selectConversation = (
    projectId: string,
    conversationId: string,
  ): void => {
    updateDesktop((current) => ({
      ...current,
      activeProjectId: projectId,
      activeConversationId: conversationId,
    }));
    closeMobileNavigation();
  };

  const toggleProject = (projectId: string): void => {
    updateDesktop((current) => ({
      ...current,
      projects: current.projects.map((project) =>
        project.id === projectId
          ? { ...project, expanded: !project.expanded }
          : project,
      ),
    }));
  };

  const chooseModel = (modelId: string): void => {
    if (!activeConversation || !activeProject || modelBusy) return;
    if (activeAgentScope?.sessionId !== activeConversation.id) {
      showNotice("The conversation is still preparing. Try again in a moment.");
      return;
    }
    const model = models.find((item) => item.id === modelId);
    const projectId = activeProject.id;
    const conversationId = activeConversation.id;
    const scope = activeAgentScope;
    void (async () => {
      setModelBusy(true);
      try {
        const agent = createAgentClient(scope);
        await agent.setModel(modelId);
        const effectiveModel = await agent.getModel();
        const config = await agent.getConfig();
        const thinkingLevel = normalizeThinkingLevel(
          config.thinkingLevel,
          model,
        );
        if (model?.supportsReasoning && thinkingLevel !== config.thinkingLevel) {
          await agent.setThinking(thinkingLevel);
        }
        if (effectiveModel !== modelId) {
          throw new Error(
            `Model switch returned "${effectiveModel}" instead of "${modelId}".`,
          );
        }
        updateDesktop((current) => ({
          ...current,
          projects: current.projects.map((project) =>
            project.id !== projectId
              ? project
              : {
                  ...project,
                  conversations: project.conversations.map((conversation) =>
                    conversation.id === conversationId
                      ? {
                          ...conversation,
                          modelId: effectiveModel,
                          thinkingLevel,
                        }
                      : conversation,
                  ),
                },
          ),
        }));
        await setDefaultModel(effectiveModel);
        setModels((current) =>
          current.map((item) => ({
            ...item,
            isDefault: item.id === effectiveModel,
          })),
        );
      } catch (error) {
        showNotice(conciseError(error));
      } finally {
        setModelBusy(false);
      }
    })();
  };

  const choosePermissionMode = (mode: PermissionMode): void => {
    if (!activeConversation || !activeProject) return;
    if (activeAgentScope?.sessionId !== activeConversation.id) {
      showNotice("The conversation is still preparing. Try again in a moment.");
      return;
    }
    const projectId = activeProject.id;
    const conversationId = activeConversation.id;
    const scope = activeAgentScope;
    void createAgentClient(scope)
      .setPermission(mode)
      .then(() => {
        updateDesktop((current) => ({
          ...current,
          projects: current.projects.map((project) =>
            project.id !== projectId
              ? project
              : {
                  ...project,
                  conversations: project.conversations.map((conversation) =>
                    conversation.id === conversationId
                      ? { ...conversation, permissionMode: mode }
                      : conversation,
                  ),
                },
          ),
        }));
      })
      .catch((error) => showNotice(conciseError(error)));
  };

  const renameConversation = (nextTitle: string): void => {
    if (!activeConversation || !activeProject) return;
    if (activeAgentScope?.sessionId !== activeConversation.id) {
      showNotice(t("notice.sessionPreparing"));
      return;
    }
    const projectId = activeProject.id;
    const conversationId = activeConversation.id;
    const scope = activeAgentScope;
    void createAgentClient(scope)
      .renameSession(nextTitle)
      .then(() => {
        updateDesktop((current) => ({
          ...current,
          projects: current.projects.map((project) =>
            project.id !== projectId
              ? project
              : {
                  ...project,
                  conversations: project.conversations.map((conversation) =>
                    conversation.id === conversationId
                      ? { ...conversation, title: nextTitle }
                      : conversation,
                  ),
                },
          ),
        }));
      })
      .catch((error) => showNotice(conciseError(error)));
  };

  const chooseEffort = (level: string): void => {
    if (!activeConversation || !activeProject || modelBusy) return;
    if (!thinkingLevelsForModel(selectedModel).includes(level)) return;
    if (activeAgentScope?.sessionId !== activeConversation.id) {
      showNotice("The conversation is still preparing. Try again in a moment.");
      return;
    }
    const projectId = activeProject.id;
    const conversationId = activeConversation.id;
    const scope = activeAgentScope;
    void createAgentClient(scope)
      .setThinking(level)
      .then(() => {
        updateDesktop((current) => ({
          ...current,
          projects: current.projects.map((project) =>
            project.id !== projectId
              ? project
              : {
                  ...project,
                  conversations: project.conversations.map((conversation) =>
                    conversation.id === conversationId
                      ? { ...conversation, thinkingLevel: level }
                      : conversation,
                  ),
                },
          ),
        }));
      })
      .catch((error) => showNotice(conciseError(error)));
  };

  const togglePlanMode = async (): Promise<void> => {
    if (!activeAgentScope || modeBusy || isStreaming) return;
    setModeBusy(true);
    try {
      const agent = createAgentClient(activeAgentScope);
      if (activePlan) {
        await agent.cancelPlan(activePlan.id);
      } else {
        await agent.enterPlan();
      }
      await refreshAgentState(activeAgentScope);
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setModeBusy(false);
    }
  };

  const startLogin = async (): Promise<void> => {
    setProfileOpen(false);
    setLoginOpen(true);
    setLoginBusy(true);
    setDeviceCode(undefined);
    try {
      const status = await invoke<AuthStatus>("login");
      setAuth(status);
      if (status.loggedIn) {
        setLoginOpen(false);
        showNotice(t("notice.loginSuccess"));
        void refreshModels();
      }
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setLoginBusy(false);
    }
  };

  const signOut = async (): Promise<void> => {
    try {
      const status = await invoke<AuthStatus>("logout");
      setAuth(status);
      accountUsageRequest.current += 1;
      setAccountUsage(undefined);
      setAccountUsageBusy(false);
      setAccountUsageError(undefined);
      setProfileOpen(false);
      showNotice(t("notice.logoutSuccess"));
    } catch (error) {
      showNotice(conciseError(error));
    }
  };

  const refreshHistory = async (
    conversationId: string,
    completedTurn?: InFlightTurn,
  ): Promise<boolean> => {
    const request = (historyRequests.current[conversationId] ?? 0) + 1;
    historyRequests.current[conversationId] = request;
    try {
      const page = await fetchConversationHistory(conversationId);
      if (request !== historyRequests.current[conversationId]) return false;
      const items = [...page.items].reverse();
      const durationMs = completedTurn?.durationMs;
      if (completedTurn && durationMs !== undefined) {
        const messageId = completedTurnMessageId(items, completedTurn);
        if (messageId) {
          setMessageDurations((current) => ({
            ...current,
            [conversationId]: {
              ...current[conversationId],
              [messageId]: durationMs,
            },
          }));
        }
      }
      setHistoryByConversation((current) => ({
        ...current,
        [conversationId]: {
          conversationId,
          items,
          loading: false,
        },
      }));
      return true;
    } catch (error) {
      if (request !== historyRequests.current[conversationId]) return false;
      const message = conciseError(error);
      setHistoryByConversation((current) => ({
        ...current,
        [conversationId]: {
          conversationId,
          items: current[conversationId]?.items ?? [],
          loading: false,
          error: message,
        },
      }));
      showNotice(message);
      return false;
    }
  };

  useEffect(() => {
    const conversationId = activeConversation?.id;
    if (!conversationId || activeCompaction?.phase !== "completed") return;
    void refreshHistory(conversationId).then((refreshed) => {
      if (!refreshed) return;
      setCompactionHistoryReady((current) => ({
        ...current,
        [conversationId]: true,
      }));
    });
  }, [activeConversation?.id, activeCompaction?.phase]);

  const loadAvailableSkills = async (): Promise<void> => {
    const request = skillsRequest.current + 1;
    skillsRequest.current = request;
    const scope = activeAgentScope;
    if (!scope) {
      setAvailableSkills([]);
      setSkillsBusy(false);
      setSkillsError(t("notice.sessionPreparing"));
      return;
    }

    setSkillsBusy(true);
    setSkillsError(undefined);
    try {
      const skills = await listSkills(scope.sessionId);
      if (request !== skillsRequest.current) return;
      setAvailableSkills(skills);
    } catch (error) {
      if (request !== skillsRequest.current) return;
      setAvailableSkills([]);
      setSkillsError(conciseError(error));
    } finally {
      if (request === skillsRequest.current) setSkillsBusy(false);
    }
  };

  const toggleComposerAdd = (): void => {
    if (composerAddOpen) {
      setComposerAddOpen(false);
      return;
    }
    setComposerAddOpen(true);
    void loadAvailableSkills();
  };

  const selectPromptSkill = (skill: SkillDescriptor): void => {
    const selected = promptSkills.some(
      (item) => item.name === skill.name,
    );
    if (!selected && promptSkills.length >= MAX_PROMPT_SKILLS) {
      showNotice(t("notice.maxSkills", { count: MAX_PROMPT_SKILLS }));
      setComposerAddOpen(false);
      return;
    }
    setPromptSkills((current) =>
      selected
        ? current.filter((item) => item.name !== skill.name)
        : [...current, skill],
    );
    setComposerAddOpen(false);
    window.requestAnimationFrame(() => textareaRef.current?.focus());
  };

  const openSkillDetail = async (skill: SkillDetailTarget): Promise<void> => {
    const request = skillDetailRequest.current + 1;
    skillDetailRequest.current = request;
    const scope = activeAgentScope;

    setComposerAddOpen(false);
    closeSideChat();
    setCompactionSummaryDetail(undefined);
    setSkillDetailTarget(skill);
    setSkillDetail(undefined);
    setSkillDetailError(undefined);
    if (!scope) {
      setSkillDetailBusy(false);
      setSkillDetailError(t("notice.sessionPreparing"));
      return;
    }

    setSkillDetailBusy(true);
    try {
      const content = await getSkillContent(scope.sessionId, skill.name);
      if (request !== skillDetailRequest.current) return;
      setSkillDetail(content);
    } catch (error) {
      if (request !== skillDetailRequest.current) return;
      setSkillDetailError(conciseError(error));
    } finally {
      if (request === skillDetailRequest.current) setSkillDetailBusy(false);
    }
  };

  const closeSkillDetail = (): void => {
    skillDetailRequest.current += 1;
    setSkillDetailTarget(undefined);
    setSkillDetail(undefined);
    setSkillDetailBusy(false);
    setSkillDetailError(undefined);
  };

  const openCompactionSummary = (message: RenderMessage): void => {
    closeSideChat();
    skillDetailRequest.current += 1;
    setSkillDetailTarget(undefined);
    setSkillDetail(undefined);
    setSkillDetailBusy(false);
    setSkillDetailError(undefined);
    setCompactionSummaryDetail({
      id: message.id,
      content: messageText(message),
      createdAt: message.created_at,
    });
  };

  const addPromptAttachments = async (
    files: readonly File[],
  ): Promise<void> => {
    if (files.length === 0) return;
    const remaining = MAX_PROMPT_ATTACHMENTS - promptAttachments.length;
    if (remaining <= 0) {
      showNotice(t("notice.maxAttachments", { count: MAX_PROMPT_ATTACHMENTS }));
      return;
    }

    const selected = files.slice(0, remaining);
    const prepared: PromptAttachment[] = [];
    for (const file of selected) {
      try {
        const kind = promptAttachmentKind(file.type);
        if (kind === "image" && !selectedModel?.supportsImage) {
          throw new Error(t("error.imageNotSupported"));
        }
        if (kind === "video" && !selectedModel?.supportsVideo) {
          throw new Error(t("error.videoNotSupported"));
        }
        prepared.push(await preparePromptAttachment(file));
      } catch (error) {
        showNotice(conciseError(error));
      }
    }
    if (prepared.length > 0) {
      setPromptAttachments((current) => [...current, ...prepared]);
    }
    if (files.length > remaining) {
      showNotice(t("notice.maxAttachments", { count: MAX_PROMPT_ATTACHMENTS }));
    }
  };

  const handleAttachmentInput = (
    event: ChangeEvent<HTMLInputElement>,
  ): void => {
    const files = Array.from(event.target.files ?? []);
    event.target.value = "";
    void addPromptAttachments(files);
  };

  const handlePromptPaste = (
    event: ClipboardEvent<HTMLTextAreaElement>,
  ): void => {
    const media = Array.from(event.clipboardData.items)
      .filter((item) => item.kind === "file")
      .map((item) => item.getAsFile())
      .filter((file): file is File => file !== null);
    if (media.length > 0) void addPromptAttachments(media);
  };

  const sendPrompt = async (
    override?: string,
    queuedAttachments?: readonly PromptAttachment[],
    queuedSkills?: readonly SkillDescriptor[],
  ): Promise<void> => {
    const text = (override ?? prompt).trim();
    const attachments = [
      ...(queuedAttachments === undefined
        ? promptAttachments
        : queuedAttachments),
    ];
    const skills = [
      ...(queuedSkills === undefined ? promptSkills : queuedSkills),
    ];
    const submittedText = buildSkillPromptText(text, skills);
    if (
      (!submittedText && attachments.length === 0) ||
      !activeProject ||
      !activeConversation ||
      modelBusy ||
      isHistoryLoading ||
      hasBlockingInteraction
    ) {
      return;
    }
    if (!selectedModel) {
      showNotice(t("notice.modelRequired"));
      return;
    }
    if (
      attachments.some((attachment) => attachment.kind === "image") &&
      !selectedModel.supportsImage
    ) {
      showNotice(t("error.imageNotSupported"));
      return;
    }
    if (
      attachments.some((attachment) => attachment.kind === "video") &&
      !selectedModel.supportsVideo
    ) {
      showNotice(t("error.videoNotSupported"));
      return;
    }

    const conversationId = activeConversation.id;
    const projectId = activeProject.id;
    if (activeAgentScope?.sessionId !== conversationId) {
      showNotice(t("notice.sessionPreparing"));
      return;
    }

    if (isStreaming) {
      const queued: QueuedPrompt = {
        id: newQueuedPromptId(),
        text,
        attachments,
        skills,
        createdAt: new Date().toISOString(),
      };
      setQueuedPrompts((current) => ({
        ...current,
        [conversationId]: [...(current[conversationId] ?? []), queued],
      }));
      updateDesktop((current) => ({
        ...current,
        projects: current.projects.map((project) =>
          project.id !== activeProject.id
            ? project
            : {
                ...project,
                conversations: project.conversations.map((conversation) =>
                  conversation.id === conversationId
                    ? { ...conversation, updatedAt: Date.now() }
                    : conversation,
                ),
              },
        ),
      }));
      if (queuedAttachments === undefined) {
        resetPrompt();
        setPromptAttachments([]);
        setPromptSkills([]);
      }
      followLatestMessageRef.current = true;
      return;
    }

    const title =
      activeConversation.title === t("conversation.new")
        ? (
            submittedText ||
            t("conversation.mediaTitle", { count: attachments.length })
          )
            .replace(/\s+/g, " ")
            .slice(0, 28)
        : activeConversation.title;
    const input = buildAgentPromptInput(text, attachments);

    followLatestMessageRef.current = true;
    setCompactions((current) => {
      if (!(conversationId in current)) return current;
      const next = { ...current };
      delete next[conversationId];
      return next;
    });
    setInFlightTurns((current) => ({
      ...current,
      [conversationId]: newInFlightTurn(
        text,
        attachments,
        activeHistory?.items.at(-1)?.id,
        skills.map((skill) => skill.name),
      ),
    }));
    updateDesktop((current) => ({
      ...current,
      projects: current.projects.map((project) =>
        project.id !== projectId
          ? project
          : {
              ...project,
              conversations: project.conversations.map((conversation) =>
                conversation.id !== conversationId
                  ? conversation
                  : {
                      ...conversation,
                      title,
                      modelId: selectedModel.id,
                      updatedAt: Date.now(),
                    },
              ),
            },
      ),
    }));
    if (queuedAttachments === undefined) {
      resetPrompt();
      setPromptAttachments([]);
      setPromptSkills([]);
    }

    try {
      const client = createAgentClient(activeAgentScope);
      const submitted = await client.prompt(input, {
        skills: skills.map((skill) => ({ name: skill.name })),
      });
      setInFlightTurns((current) => {
        const turn = current[conversationId];
        if (!turn) return current;
        const status = liveTurnStatusFromSubmit(submitted.status);
        if (
          submitted.turnId !== undefined &&
          turn.turnId === submitted.turnId &&
          !isTurnRunning(turn) &&
          (status === "queued" || status === "running")
        ) {
          return current;
        }
        return {
          ...current,
          [conversationId]: {
            ...turn,
            promptId: submitted.promptId,
            turnId: submitted.turnId ?? turn.turnId,
            status,
            durationMs:
              isTurnRunning({ ...turn, status })
                ? turn.durationMs
                : (turn.durationMs ??
                  Math.max(0, Date.now() - Date.parse(turn.createdAt))),
          },
        };
      });
    } catch (error) {
      const message = conciseError(error);
      setInFlightTurns((current) => {
        const turn = current[conversationId];
        if (!turn) return current;
        return {
          ...current,
          [conversationId]: {
            ...turn,
            status: "failed",
            durationMs: Math.max(0, Date.now() - Date.parse(turn.createdAt)),
            error: message,
          },
        };
      });
      showNotice(message);
    }

  };

  const removeQueuedPrompt = (queuedPromptId: string): void => {
    if (!activeConversation) return;
    const conversationId = activeConversation.id;
    setQueuedPrompts((current) => ({
      ...current,
      [conversationId]: (current[conversationId] ?? []).filter(
        (item) => item.id !== queuedPromptId || item.steering,
      ),
    }));
  };

  const steerQueuedPrompt = async (queuedPromptId: string): Promise<void> => {
    if (!activeConversation || !activeAgentScope || !isStreaming) return;
    const conversationId = activeConversation.id;
    const queued = activeQueuedPrompts.find(
      (item) => item.id === queuedPromptId,
    );
    if (!queued || queued.steering || queued.skills.length > 0) return;

    setQueuedPrompts((current) => ({
      ...current,
      [conversationId]: (current[conversationId] ?? []).map((item) =>
        item.id === queuedPromptId ? { ...item, steering: true } : item,
      ),
    }));

    try {
      const submitted = await createAgentClient(activeAgentScope).steer(
        buildAgentPromptInput(
          buildSkillPromptText(queued.text, queued.skills),
          queued.attachments,
        ),
      );
      if (submitted.status === "steered") {
        setInFlightTurns((current) => {
          const turn = current[conversationId];
          if (!turn) return current;
          const placement = turn.steeredPrompts.find(
            (item) => item.promptId === submitted.promptId,
          );
          return {
            ...current,
            [conversationId]: {
              ...turn,
              steeredPrompts: placement
                ? turn.steeredPrompts.map((item) =>
                    item.promptId === submitted.promptId
                      ? {
                          ...item,
                          message: { ...queued, steering: false },
                        }
                      : item,
                  )
                : [
                    ...turn.steeredPrompts,
                    {
                      promptId: submitted.promptId,
                      message: { ...queued, steering: false },
                    },
                  ],
            },
          };
        });
      } else {
        const turn = newInFlightTurn(
          buildSkillPromptText(queued.text, queued.skills),
          queued.attachments,
          activeHistory?.items.at(-1)?.id,
        );
        setInFlightTurns((current) => ({
          ...current,
          [conversationId]: {
            ...turn,
            promptId: submitted.promptId,
            turnId: submitted.turnId,
            status: liveTurnStatusFromSubmit(submitted.status),
          },
        }));
      }
      setQueuedPrompts((current) => ({
        ...current,
        [conversationId]: (current[conversationId] ?? []).filter(
          (item) => item.id !== queuedPromptId,
        ),
      }));
      showNotice(
        submitted.status === "steered"
          ? t("notice.steeredNow")
          : t("notice.steeredNext"),
      );
    } catch (error) {
      setQueuedPrompts((current) => ({
        ...current,
        [conversationId]: (current[conversationId] ?? []).map((item) =>
          item.id === queuedPromptId ? { ...item, steering: false } : item,
        ),
      }));
      showNotice(conciseError(error));
    }
  };

  useEffect(() => {
    const conversationId = activeConversation?.id;
    const status = activeTurn?.status;
    if (
      !conversationId ||
      !status ||
      status === "queued" ||
      status === "running" ||
      status === "failed" ||
      status === "blocked"
    ) {
      return;
    }
    let active = true;
    let handoffTimer: number | undefined;
    void refreshHistory(conversationId, activeTurn).then((refreshed) => {
      if (!active || !refreshed) return;
      handoffTimer = window.setTimeout(() => {
        setInFlightTurns((current) => {
          if (!(conversationId in current)) return current;
          const next = { ...current };
          delete next[conversationId];
          return next;
        });
      }, LIVE_TURN_HANDOFF_MS);
    });
    return () => {
      active = false;
      if (handoffTimer !== undefined) window.clearTimeout(handoffTimer);
    };
  }, [activeConversation?.id, activeTurn?.status]);

  useEffect(() => {
    const conversationId = activeConversation?.id;
    const queued = activeQueuedPrompts[0];
    if (
      !conversationId ||
      !queued ||
      activeTurn !== undefined ||
      activeAgentScope?.sessionId !== conversationId ||
      isHistoryLoading ||
      modelBusy ||
      hasBlockingInteraction ||
      drainingQueuedPrompts.current.has(queued.id)
    ) {
      return;
    }

    drainingQueuedPrompts.current.add(queued.id);
    setQueuedPrompts((current) => ({
      ...current,
      [conversationId]: (current[conversationId] ?? []).filter(
        (item) => item.id !== queued.id,
      ),
    }));
    void sendPrompt(queued.text, queued.attachments, queued.skills).finally(
      () => {
        drainingQueuedPrompts.current.delete(queued.id);
      },
    );
  }, [
    activeAgentScope?.sessionId,
    activeConversation?.id,
    activeQueuedPrompts[0]?.id,
    activeTurn,
    hasBlockingInteraction,
    isHistoryLoading,
    modelBusy,
  ]);

  const handleSubmit = (event: FormEvent): void => {
    event.preventDefault();
    void sendPrompt();
  };

  const cancelActiveTurn = async (): Promise<void> => {
    if (!activeAgentScope || !activeTurn) return;
    try {
      await createAgentClient(activeAgentScope).cancel(activeTurn.turnId);
    } catch (error) {
      showNotice(conciseError(error));
    }
  };

  const runCompactionCommand = async (): Promise<void> => {
    const scope = activeAgentScope;
    if (!scope) {
      showNotice(t("notice.sessionPreparing"));
      return;
    }
    if (isStreaming) {
      showNotice(t("notice.compactWhileRunning"));
      return;
    }
    if (activeCompaction?.phase === "started" || compactionCommandBusy) {
      showNotice(t("notice.compacting"));
      return;
    }

    const nextPrompt = prompt.startsWith("/") ? prompt.slice(1) : prompt;
    resetPrompt(nextPrompt);
    setCompactionCommandBusy(true);
    try {
      await createAgentClient(scope).beginCompaction();
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setCompactionCommandBusy(false);
      window.requestAnimationFrame(() => {
        const textarea = textareaRef.current;
        if (!textarea) return;
        textarea.focus();
        textarea.setSelectionRange(0, 0);
      });
    }
  };

  const runForkCommand = async (): Promise<void> => {
    const project = activeProject;
    const source = activeConversation;
    if (
      !project ||
      !source ||
      activeAgentScope?.sessionId !== source.id
    ) {
      showNotice(t("notice.sessionPreparing"));
      return;
    }
    if (isStreaming) {
      showNotice(t("notice.forkWhileRunning"));
      return;
    }
    if (
      activeCompaction?.phase === "started" ||
      compactionCommandBusy ||
      forkCommandBusy
    ) {
      showNotice(t("notice.forkWhileCompacting"));
      return;
    }

    const nextPrompt = prompt.startsWith("/") ? prompt.slice(1) : prompt;
    resetPrompt(nextPrompt);
    setForkCommandBusy(true);
    try {
      const forkedId = await forkSession(source.id);
      const sessions = await listWorkspaceSessions(project.id).catch(
        () => [],
      );
      const summary = sessions.find((session) => session.id === forkedId);
      const forkedConversation = {
        ...(summary
          ? conversationFromSummary(summary)
          : {
              ...conversationFromSession(forkedId),
              title: `Fork: ${source.title}`,
            }),
        modelId: source.modelId,
        thinkingLevel: source.thinkingLevel,
        permissionMode: source.permissionMode,
      };

      updateDesktop((current) => ({
        ...current,
        activeProjectId: project.id,
        activeConversationId: forkedId,
        projects: current.projects.map((item) =>
          item.id === project.id
            ? {
                ...item,
                expanded: true,
                conversations: [
                  forkedConversation,
                  ...item.conversations.filter(
                    (conversation) => conversation.id !== forkedId,
                  ),
                ],
              }
            : item,
        ),
      }));
      followLatestMessageRef.current = true;
      showNotice(t("notice.forked"));
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setForkCommandBusy(false);
      window.requestAnimationFrame(() => textareaRef.current?.focus());
    }
  };

  const openSideChatCommand = (): void => {
    const conversation = activeConversation;
    if (
      !conversation ||
      activeAgentScope?.sessionId !== conversation.id
    ) {
      showNotice(t("notice.sessionPreparing"));
      return;
    }
    if (activeCompaction?.phase === "started") {
      showNotice(t("notice.sideChatWhileCompacting"));
      return;
    }

    sideChatAgentId.current = undefined;
    const instanceId = sideChatInstance.current + 1;
    sideChatInstance.current = instanceId;

    const nextPrompt = prompt.startsWith("/") ? prompt.slice(1) : prompt;
    resetPrompt(nextPrompt);
    setSlashMenuOpen(false);
    skillDetailRequest.current += 1;
    setSkillDetailTarget(undefined);
    setSkillDetail(undefined);
    setSkillDetailBusy(false);
    setSkillDetailError(undefined);
    setCompactionSummaryDetail(undefined);
    setSideChat({
      instanceId,
      parentSessionId: conversation.id,
      draft: "",
      turns: [],
      starting: false,
    });
  };

  const updateSideChatDraft = (draft: string): void => {
    setSideChat((current) =>
      current ? { ...current, draft } : current,
    );
  };

  const sendSideChatPrompt = async (): Promise<void> => {
    const current = sideChat;
    const scope = activeAgentScope;
    const text = current?.draft.trim() ?? "";
    if (
      !current ||
      !scope ||
      scope.sessionId !== current.parentSessionId ||
      !text ||
      current.starting ||
      isTurnRunning(current.turns.at(-1))
    ) {
      return;
    }

    const instanceId = current.instanceId;
    const createdAt = new Date().toISOString();
    setSideChat((value) =>
      value?.instanceId === instanceId
        ? {
            ...value,
            draft: "",
            starting: true,
            turns: [
              ...value.turns,
              { ...newInFlightTurn(text, []), createdAt },
            ],
          }
        : value,
    );

    try {
      let agentId = current.agentId;
      if (!agentId) {
        agentId = await createAgentClient(scope).startBtw();
        if (sideChatInstance.current !== instanceId) return;
        sideChatAgentIds.current.add(agentId);
        sideChatAgentId.current = agentId;
        setSideChat((value) =>
          value?.instanceId === instanceId
            ? { ...value, agentId }
            : value,
        );
      }

      const submitted = await createAgentClient({
        sessionId: current.parentSessionId,
        agentId,
      }).prompt(text);
      if (sideChatInstance.current !== instanceId) return;
      setSideChat((value) => {
        if (value?.instanceId !== instanceId) return value;
        const turns = [...value.turns];
        const last = turns.at(-1);
        if (!last || last.createdAt !== createdAt) return value;
        const status = liveTurnStatusFromSubmit(submitted.status);
        if (
          !isTurnRunning(last) &&
          (status === "queued" || status === "running")
        ) {
          return { ...value, starting: false };
        }
        turns[turns.length - 1] = {
          ...last,
          turnId: submitted.turnId ?? last.turnId,
          status,
          durationMs:
            status === "queued" || status === "running"
              ? last.durationMs
              : (last.durationMs ??
                Math.max(0, Date.now() - Date.parse(last.createdAt))),
        };
        return { ...value, turns, starting: false };
      });
    } catch (error) {
      if (sideChatInstance.current !== instanceId) return;
      const message = conciseError(error);
      setSideChat((value) => {
        if (value?.instanceId !== instanceId) return value;
        const turns = [...value.turns];
        const last = turns.at(-1);
        if (last?.createdAt === createdAt) {
          turns[turns.length - 1] = {
            ...last,
            status: "failed",
            durationMs: Math.max(
              0,
              Date.now() - Date.parse(last.createdAt),
            ),
            error: message,
          };
        }
        return { ...value, turns, starting: false };
      });
      showNotice(message);
    }
  };

  const handlePromptKeyDown = (
    event: KeyboardEvent<HTMLTextAreaElement>,
  ): void => {
    if (event.nativeEvent.isComposing || promptCompositionRef.current) return;
    if (slashMenuOpen && event.key === "Escape") {
      event.preventDefault();
      setSlashMenuOpen(false);
      return;
    }
    if (
      slashMenuOpen &&
      (event.key === "ArrowDown" || event.key === "ArrowUp")
    ) {
      event.preventDefault();
      setSlashMenuActiveIndex((current) => {
        const delta = event.key === "ArrowDown" ? 1 : -1;
        return (
          (current + delta + SLASH_COMMAND_COUNT) %
          SLASH_COMMAND_COUNT
        );
      });
      return;
    }
    if (
      slashMenuOpen &&
      event.key === "Enter" &&
      !event.shiftKey
    ) {
      event.preventDefault();
      if (slashMenuActiveIndex === 0) {
        void runCompactionCommand();
      } else if (slashMenuActiveIndex === 1) {
        void runForkCommand();
      } else {
        openSideChatCommand();
      }
      return;
    }
    if (
      event.key.toLowerCase() === "z" &&
      (event.ctrlKey || event.metaKey) &&
      !event.altKey &&
      !event.shiftKey
    ) {
      event.preventDefault();
      if (canUndoPromptEdit(promptUndoHistoryRef.current)) {
        undoPrompt();
      }
      return;
    }
    if (
      event.key === "Backspace" &&
      prompt.length === 0 &&
      promptSkills.length > 0
    ) {
      event.preventDefault();
      setPromptSkills((current) => current.slice(0, -1));
      return;
    }
    if (
      event.key === "Enter" &&
      !event.shiftKey &&
      !event.nativeEvent.isComposing
    ) {
      event.preventDefault();
      void sendPrompt();
    }
  };

  const copyMessage = useCallback(async (message: ProtocolMessage): Promise<void> => {
    const text = displayMessageText(message);
    if (!text) return;
    await navigator.clipboard.writeText(text);
    setCopiedMessage(message.id);
    window.setTimeout(() => setCopiedMessage(undefined), 1400);
  }, []);

  const respondToInteraction = async (
    interaction: AgentInteraction,
    response: unknown,
  ): Promise<void> => {
    if (!activeConversation || resolvingInteraction) return;
    setResolvingInteraction(interaction.id);
    try {
      await invoke("respond_interaction", {
        sessionId: activeConversation.id,
        interactionId: interaction.id,
        response,
      });
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setResolvingInteraction(undefined);
    }
  };

  const resolveApproval = (
    interaction: AgentInteraction,
    decision: "approved" | "rejected",
    session = false,
  ): Promise<void> =>
    respondToInteraction(interaction, {
      decision,
      ...(session ? { scope: "session" } : {}),
      selectedLabel:
        decision === "rejected"
          ? "Reject"
          : session
            ? "Approve for this session"
            : "Approve once",
    });

  return (
    <div
      className={[
        "app-shell",
        desktopRuntime ? "desktop-runtime" : "web-runtime",
        mobileLayout ? "mobile-layout" : undefined,
        sidebarCollapsed ? "sidebar-is-collapsed" : undefined,
      ]
        .filter(Boolean)
        .join(" ")}
      style={
        mobileLayout && mobileViewportHeight
          ? ({
              "--app-viewport-height": `${mobileViewportHeight}px`,
            } as CSSProperties)
          : undefined
      }
    >
      <WindowTitleBar />

      <div className="app-body">
        <aside
          className={sidebarCollapsed ? "sidebar collapsed" : "sidebar"}
          aria-hidden={mobileLayout && !mobileSidebarOpen}
          inert={mobileLayout && !mobileSidebarOpen}
        >
        <div className="brand-row">
          <div className="sidebar-heading-copy" aria-hidden={sidebarCollapsed}>
            <strong>{t("sidebar.workspace")}</strong>
          </div>
          <button
            className="icon-button quiet"
            type="button"
            aria-label={
              sidebarCollapsed ? t("sidebar.expand") : t("sidebar.collapse")
            }
            aria-expanded={!sidebarCollapsed}
            onClick={toggleSidebar}
            title={sidebarCollapsed ? t("sidebar.expand") : t("sidebar.collapse")}
          >
            {sidebarCollapsed ? (
              <PanelLeftOpen size={17} />
            ) : (
              <PanelLeftClose size={17} />
            )}
          </button>
        </div>

        <div className="sidebar-primary">
          <button className="new-project-button" onClick={() => void addProject()}>
            <Plus size={17} />
            <span className="sidebar-control-label" aria-hidden={sidebarCollapsed}>
              {t("sidebar.openProject")}
            </span>
          </button>

          <div className="sidebar-section-heading" aria-hidden={sidebarCollapsed}>
            <span>{t("sidebar.projects")}</span>
          </div>

          <nav className="project-list" aria-label={t("sidebar.projectsAndConversations")}>
            {desktop.projects.map((project) => {
              const isProjectActive = project.id === activeProject?.id;
              return (
                <div
                  className={`project-group ${isProjectActive ? "active" : ""}`}
                  key={project.id}
                >
                  <div
                    className="project-row"
                    onClick={() =>
                      sidebarCollapsed
                        ? openSidebar()
                        : toggleProject(project.id)
                    }
                    title={project.path}
                  >
                    <span
                      className="project-glyph"
                      style={{ "--project-accent": project.accent } as React.CSSProperties}
                    >
                      <FolderGit2 size={16} />
                    </span>
                    <span className="project-name" aria-hidden={sidebarCollapsed}>
                      {project.name}
                    </span>
                    <span className="project-actions" aria-hidden={sidebarCollapsed}>
                      <button
                        className="icon-button tiny"
                        type="button"
                        tabIndex={sidebarCollapsed ? -1 : 0}
                        onClick={(event) =>
                          void createConversation(project, event)
                        }
                        title={t("conversation.create")}
                        aria-label={t("conversation.newIn", { name: project.name })}
                      >
                        <Plus size={14} />
                      </button>
                      <button
                        className="icon-button tiny project-remove-button"
                        type="button"
                        tabIndex={sidebarCollapsed ? -1 : 0}
                        onClick={(event) => {
                          event.stopPropagation();
                          setRemovalTarget({
                            kind: "project",
                            projectId: project.id,
                            name: project.name,
                            path: project.path,
                            conversationIds: project.conversations.map(
                              (conversation) => conversation.id,
                            ),
                          });
                        }}
                        title={t("sidebar.removeProject")}
                        aria-label={t("sidebar.removeProjectNamed", { name: project.name })}
                      >
                        <FolderMinus size={13} />
                      </button>
                      <ChevronRight
                        className={`project-chevron ${
                          project.expanded ? "expanded" : ""
                        }`}
                        size={14}
                      />
                    </span>
                  </div>
                  <div
                    className={`conversation-list-collapse ${
                      !sidebarCollapsed && project.expanded ? "expanded" : ""
                    }`}
                    aria-hidden={sidebarCollapsed || !project.expanded}
                    inert={sidebarCollapsed || !project.expanded}
                  >
                    <div className="conversation-list-clip">
                      <div className="conversation-list">
                      {project.conversations.map((conversation) => (
                        <div
                          className={`conversation-row ${
                            conversation.id === activeConversation?.id
                              ? "selected"
                              : ""
                          }`}
                          key={conversation.id}
                        >
                          <button
                            className="conversation-select"
                            type="button"
                            onClick={() =>
                              selectConversation(project.id, conversation.id)
                            }
                            title={conversation.title}
                          >
                            <MessageSquareText size={14} />
                            <span className="conversation-title">
                              {conversation.title}
                            </span>
                            {isTurnRunning(inFlightTurns[conversation.id]) && (
                              <span className="conversation-meta">
                                <span
                                  className="conversation-running-indicator"
                                  role="status"
                                  aria-label={t("conversation.running")}
                                  title={t("conversation.running")}
                                />
                              </span>
                            )}
                          </button>
                          <button
                            className="conversation-archive-button"
                            type="button"
                            onClick={(event) => {
                              event.stopPropagation();
                              setRemovalTarget({
                                kind: "conversation",
                                projectId: project.id,
                                conversationId: conversation.id,
                                title: conversation.title,
                              });
                            }}
                            title={t("conversation.archive")}
                            aria-label={t("conversation.archiveNamed", { title: conversation.title })}
                          >
                            <Archive size={12} />
                          </button>
                        </div>
                      ))}
                      </div>
                    </div>
                  </div>
                </div>
              );
            })}
          </nav>

          {desktop.projects.length === 0 && (
            <div className="sidebar-empty" aria-hidden={sidebarCollapsed}>
              <Folder size={22} />
              <p>{t("sidebar.empty")}</p>
            </div>
          )}
        </div>

        <div className="account-area">
          <div className="profile-wrap" ref={profileRef}>
            <button
              className={auth.loggedIn ? "account-button" : "account-button login"}
              tabIndex={sidebarCollapsed ? -1 : 0}
              aria-label={t("account.openMenu")}
              aria-expanded={profileOpen}
              aria-controls="account-popover"
              onClick={toggleProfile}
            >
              <span className={auth.loggedIn ? "avatar" : "avatar signed-out"}>
                {auth.loggedIn ? (
                  <Sparkles size={15} />
                ) : (
                  <CircleUserRound size={18} />
                )}
                {auth.loggedIn && <i />}
              </span>
              <span className="account-copy" aria-hidden={sidebarCollapsed}>
                <strong>
                  {auth.loggedIn ? "Kimi Code" : t("account.login")}
                </strong>
                <small>
                  {auth.loggedIn
                    ? t("account.connected")
                    : t("account.loginHint")}
                </small>
              </span>
              {auth.loggedIn ? (
                <MoreHorizontal
                  className="account-trailing-icon"
                  size={16}
                  aria-hidden={sidebarCollapsed}
                />
              ) : (
                <LogIn
                  className="account-trailing-icon"
                  size={16}
                  aria-hidden={sidebarCollapsed}
                />
              )}
            </button>
            <div
              className="account-compact-actions"
              aria-hidden={!sidebarCollapsed}
              inert={!sidebarCollapsed}
            >
              <button
                className="account-compact-kimi"
                type="button"
                title={t("account.kimiAccount")}
                aria-label={t("account.openMenu")}
                aria-expanded={profileOpen}
                aria-controls="account-popover"
                onClick={toggleProfile}
              >
                {auth.loggedIn ? (
                  <Sparkles size={14} />
                ) : (
                  <CircleUserRound size={15} />
                )}
              </button>
            </div>
            {profileOpen && (
              <AccountUsagePopover
                appVersion={appVersion}
                loggedIn={auth.loggedIn}
                usage={accountUsage}
                busy={accountUsageBusy}
                error={accountUsageError}
                onRefresh={() => void loadAccountUsage()}
                onLogin={() => void startLogin()}
                onOpenSettings={openSettings}
                onSignOut={() => void signOut()}
              />
            )}
          </div>
        </div>
        </aside>

        {mobileLayout && mobileSidebarOpen && (
          <button
            className="mobile-sidebar-backdrop"
            type="button"
            aria-label={t("sidebar.collapse")}
            onClick={closeMobileNavigation}
          />
        )}

        <main
          className="workspace"
          inert={mobileLayout && mobileSidebarOpen}
        >
        {activeProject && activeConversation ? (
          <>
            <header className="chat-header">
              <div className="chat-heading">
                {sidebarCollapsed && (
                  <button
                    className="icon-button"
                    ref={mobileMenuButtonRef}
                    type="button"
                    aria-label={t("sidebar.expand")}
                    aria-expanded={mobileSidebarOpen}
                    onClick={openSidebar}
                  >
                    <Menu size={18} />
                  </button>
                )}
                <div>
                  <ChatHeaderTitle
                    title={activeConversation.title}
                    onRename={renameConversation}
                  />
                  <div className="path-line">
                    <Folder size={12} />
                    <span>{activeProject.path}</span>
                  </div>
                </div>
              </div>
              <div className="header-actions">
                <button className="icon-button" title={t("conversation.create")} onClick={() => void createConversation(activeProject)}>
                  <SquarePen size={17} />
                </button>
              </div>
            </header>

            <ConversationOutline
              items={conversationOutlineItems}
              activeTurnId={activeOutlineTurnId}
              hidden={isHistoryLoading}
              onSelect={scrollToConversationTurn}
            />

            <div
              className="chat-scroll"
              ref={scrollRef}
              onScroll={handleChatScroll}
            >
              {isHistoryLoading ? (
                <div className="history-loading">
                  <span className="spinner" />
                  {t("history.loading")}
                </div>
              ) : activeHistory?.error && !hasVisibleMessages ? (
                <div className="history-loading error">
                  {activeHistory.error}
                </div>
              ) : !hasVisibleMessages ? (
                <Welcome
                  project={activeProject}
                  onSuggestion={(value) => void sendPrompt(value)}
                />
              ) : (
                <div className="message-stack" ref={messageStackRef}>
                  {activeHistory?.error && (
                    <div className="history-error">{activeHistory.error}</div>
                  )}
                  {historyConversationTurns.map((turn) => (
                    <HistoryTurnView
                      key={turn.id}
                      turn={turn}
                      toolResults={historyToolPresentation.results}
                      subagentRuns={activeSubagentRuns}
                      subagentLiveTurns={activeSubagentLiveTurns}
                      messageDurations={
                        messageDurations[activeConversation.id] ?? {}
                      }
                      copiedMessageId={copiedMessage}
                      onCopy={copyMessage}
                      onSkillOpen={(name) =>
                        void openSkillDetail({ name })
                      }
                      onCompactionSummaryOpen={openCompactionSummary}
                      compactionEvent={
                        latestHistoryCompactionSummaryId &&
                        turn.responses.some(
                          (message) =>
                            message.id === latestHistoryCompactionSummaryId,
                        )
                          ? activeCompaction
                          : undefined
                      }
                    />
                  ))}
                  {activeTurn && (
                    <LiveTurnView
                      turn={activeTurn}
                      outlineId={liveOutlineTurnId}
                      subagentRuns={activeSubagentRuns}
                      subagentLiveTurns={activeSubagentLiveTurns}
                      onSkillOpen={(name) =>
                        void openSkillDetail({ name })
                      }
                    />
                  )}
                  {activeQueuedPrompts.length > 0 && (
                    <QueuedPromptList
                      prompts={activeQueuedPrompts}
                      canSteer={isStreaming}
                      onRemove={removeQueuedPrompt}
                      onSteer={(queuedPromptId) =>
                        void steerQueuedPrompt(queuedPromptId)
                      }
                      onSkillOpen={(name) =>
                        void openSkillDetail({ name })
                      }
                    />
                  )}
                  {activeRemoteQueuedPrompts.length > 0 && (
                    <RemoteQueuedPromptList
                      prompts={activeRemoteQueuedPrompts}
                      onSkillOpen={(name) =>
                        void openSkillDetail({ name })
                      }
                    />
                  )}
                  {activeCompaction &&
                    (activeCompaction.phase !== "completed" ||
                      !compactionHistoryReady[activeConversation.id]) && (
                    <CompactionNotice event={activeCompaction} />
                  )}
                </div>
              )}
            </div>

            <div className="composer-dock">
              {activeQuestion && (
                <QuestionCard
                  key={activeQuestion.id}
                  interaction={activeQuestion}
                  busy={resolvingInteraction === activeQuestion.id}
                  onRespond={(response) =>
                    void respondToInteraction(activeQuestion, response)
                  }
                />
              )}
              {activeApproval && isPlanReviewInteraction(activeApproval) ? (
                <PlanReviewCard
                  key={activeApproval.id}
                  interaction={activeApproval}
                  busy={resolvingInteraction === activeApproval.id}
                  onRespond={(response) =>
                    void respondToInteraction(activeApproval, response)
                  }
                />
              ) : activeApproval ? (
                <ApprovalCard
                  interaction={activeApproval}
                  busy={resolvingInteraction === activeApproval.id}
                  onReject={() =>
                    void resolveApproval(activeApproval, "rejected")
                  }
                  onApprove={() =>
                    void resolveApproval(activeApproval, "approved")
                  }
                  onApproveSession={() =>
                    void resolveApproval(activeApproval, "approved", true)
                  }
                />
              ) : null}
              {(activeBackgroundTasks.length > 0 ||
                activeTodos.some((todo) => todo.status !== "done")) && (
                <div className="composer-progress-row">
                  {activeBackgroundTasks.length > 0 && (
                    <BackgroundTaskProgress
                      tasks={activeBackgroundTasks}
                      onLoadOutput={(taskId) =>
                        activeAgentScope
                          ? loadBackgroundTaskOutput(
                              activeAgentScope,
                              taskId,
                              BACKGROUND_TASK_DETAIL_TAIL,
                            )
                          : Promise.resolve()
                      }
                    />
                  )}
                  {activeTodos.some((todo) => todo.status !== "done") && (
                    <TodoProgress todos={activeTodos} />
                  )}
                </div>
              )}
              <form className="composer" onSubmit={handleSubmit}>
                {slashMenuOpen && (
                  <div
                    className="slash-command-menu"
                    id="slash-command-menu"
                    role="menu"
                    aria-label={t("slash.commands")}
                    onMouseDown={(event) => event.preventDefault()}
                  >
                    <button
                      className={
                        slashMenuActiveIndex === 0 ? "selected" : undefined
                      }
                      id="slash-command-compact"
                      type="button"
                      role="menuitem"
                      disabled={!canRunCompaction}
                      onMouseEnter={() => setSlashMenuActiveIndex(0)}
                      onClick={() => void runCompactionCommand()}
                    >
                      <span className="slash-command-icon" aria-hidden="true">
                        {activeCompaction?.phase === "started" ? (
                          <span className="spinner" />
                        ) : (
                          <Minimize2 size={14} />
                        )}
                      </span>
                      <strong>{t("slash.compact")}</strong>
                      <small>
                        {activeCompaction?.phase === "started"
                          ? t("slash.compacting")
                          : activeContextPercent === undefined
                            ? t("slash.compactDesc")
                            : t("slash.compactDescPercent", { percent: activeContextPercent })}
                      </small>
                    </button>
                    <button
                      className={
                        slashMenuActiveIndex === 1 ? "selected" : undefined
                      }
                      id="slash-command-fork"
                      type="button"
                      role="menuitem"
                      disabled={!canRunFork}
                      onMouseEnter={() => setSlashMenuActiveIndex(1)}
                      onClick={() => void runForkCommand()}
                    >
                      <span className="slash-command-icon" aria-hidden="true">
                        {forkCommandBusy ? (
                          <span className="spinner" />
                        ) : (
                          <Copy size={14} />
                        )}
                      </span>
                      <strong>{t("slash.fork")}</strong>
                      <small>{t("slash.forkDesc")}</small>
                    </button>
                    <button
                      className={
                        slashMenuActiveIndex === 2 ? "selected" : undefined
                      }
                      id="slash-command-btw"
                      type="button"
                      role="menuitem"
                      disabled={!canOpenSideChat}
                      onMouseEnter={() => setSlashMenuActiveIndex(2)}
                      onClick={openSideChatCommand}
                    >
                      <span className="slash-command-icon" aria-hidden="true">
                        <MessageSquareText size={14} />
                      </span>
                      <strong>{t("sideChat.title")}</strong>
                      <small>{t("slash.sideChatDesc")}</small>
                    </button>
                  </div>
                )}
                {promptAttachments.length > 0 && (
                  <div className="prompt-attachment-list">
                    {promptAttachments.map((attachment) => (
                      <figure
                        className={`prompt-attachment ${attachment.kind}`}
                        key={attachment.id}
                      >
                        {attachment.kind === "image" ? (
                          <img
                            src={attachment.dataUrl}
                            alt={attachment.name}
                          />
                        ) : attachment.kind === "audio" ? (
                          <audio
                            src={attachment.dataUrl}
                            controls
                            preload="metadata"
                          />
                        ) : attachment.kind === "video" ? (
                          <video
                            src={attachment.dataUrl}
                            controls
                            preload="metadata"
                          />
                        ) : (
                          <div className="prompt-file-preview">
                            <FileCode2 size={24} />
                            <small>{formatBytes(attachment.size)}</small>
                          </div>
                        )}
                        <figcaption title={attachment.name}>
                          {attachment.name}
                        </figcaption>
                        <button
                          type="button"
                          aria-label={t("composer.removeAttachmentNamed", { name: attachment.name })}
                          title={t("composer.removeAttachment")}
                          onClick={() =>
                            setPromptAttachments((current) =>
                              current.filter(
                                (item) => item.id !== attachment.id,
                              ),
                            )
                          }
                        >
                          <X size={12} />
                        </button>
                      </figure>
                    ))}
                  </div>
                )}
                {(activePlan || promptSkills.length > 0) && (
                  <div
                    className="prompt-skill-list"
                    aria-label={t("composer.inputSettings")}
                  >
                    {activePlan && (
                      <span className="prompt-skill-chip prompt-plan-chip">
                        <span className="prompt-skill-open prompt-plan-label">
                          <ClipboardList size={13} />
                          <span>{t("plan.label")}</span>
                        </span>
                        <button
                          className="prompt-skill-remove"
                          type="button"
                          aria-label={t("plan.exit")}
                          title={t("plan.exit")}
                          disabled={modeBusy || isStreaming}
                          onClick={() => void togglePlanMode()}
                        >
                          {modeBusy ? (
                            <span className="spinner" />
                          ) : (
                            <X size={11} />
                          )}
                        </button>
                      </span>
                    )}
                    {promptSkills.map((skill) => (
                      <span className="prompt-skill-chip" key={skill.name}>
                        <button
                          className="prompt-skill-open"
                          type="button"
                          aria-label={t("skills.viewSkill", { name: skill.name })}
                          title={t("skills.viewDetail")}
                          onClick={() => void openSkillDetail(skill)}
                        >
                          <Package size={13} />
                          <span>{skill.name}</span>
                        </button>
                        <button
                          className="prompt-skill-remove"
                          type="button"
                          aria-label={t("skills.removeSkill", { name: skill.name })}
                          title={t("skills.remove")}
                          onClick={() =>
                            setPromptSkills((current) =>
                              current.filter(
                                (item) => item.name !== skill.name,
                              ),
                            )
                          }
                        >
                          <X size={11} />
                        </button>
                      </span>
                    ))}
                  </div>
                )}
                <input
                  ref={attachmentInputRef}
                  className="prompt-attachment-input"
                  type="file"
                  multiple
                  onChange={handleAttachmentInput}
                />
                <textarea
                  ref={textareaRef}
                  value={prompt}
                  onChange={(event) => {
                    updatePrompt(
                      event.target.value,
                      promptCompositionRef.current ||
                        (event.nativeEvent as InputEvent).isComposing,
                    );
                    syncSlashMenu(event.currentTarget);
                  }}
                  onCompositionStart={() => {
                    promptCompositionRef.current = true;
                  }}
                  onCompositionEnd={() => {
                    promptCompositionRef.current = false;
                    window.requestAnimationFrame(() => {
                      const textarea = textareaRef.current;
                      if (textarea) {
                        updatePrompt(textarea.value);
                        syncSlashMenu(textarea);
                      }
                    });
                  }}
                  onFocus={(event) => syncSlashMenu(event.currentTarget)}
                  onSelect={(event) => syncSlashMenu(event.currentTarget)}
                  onBlur={() => setSlashMenuOpen(false)}
                  onKeyDown={handlePromptKeyDown}
                  onPaste={handlePromptPaste}
                  aria-expanded={slashMenuOpen}
                  aria-controls={
                    slashMenuOpen ? "slash-command-menu" : undefined
                  }
                  aria-activedescendant={
                    slashMenuOpen
                      ? slashMenuActiveIndex === 0
                        ? "slash-command-compact"
                        : slashMenuActiveIndex === 1
                          ? "slash-command-fork"
                          : "slash-command-btw"
                      : undefined
                  }
                  placeholder={
                    activePlan
                      ? t("composer.placeholderPlan")
                      : isStreaming
                        ? t("composer.placeholderStreaming")
                        : t("composer.placeholder")
                  }
                  rows={1}
                  disabled={modelBusy || hasBlockingInteraction}
                />
                <div className="composer-toolbar">
                  <div className="composer-options">
                    <div
                      className={`composer-add-menu-wrap ${
                        composerAddOpen ? "open" : ""
                      }`}
                      ref={composerAddRef}
                    >
                      <button
                        className="toolbar-icon composer-add-trigger"
                        type="button"
                        title={t("composer.add")}
                        aria-label={t("composer.add")}
                        aria-expanded={composerAddOpen}
                        aria-controls="composer-add-menu"
                        onClick={toggleComposerAdd}
                        disabled={!selectedModel || modelBusy}
                      >
                        <Plus size={15} />
                      </button>
                      {composerAddOpen && (
                        <div
                          className="composer-add-menu"
                          id="composer-add-menu"
                          role="menu"
                          aria-label={t("composer.addMenu")}
                        >
                          <div className="composer-add-group">
                            <button
                              className="composer-add-item"
                              type="button"
                              role="menuitem"
                              disabled={
                                promptAttachments.length >=
                                MAX_PROMPT_ATTACHMENTS
                              }
                              onClick={() => {
                                setComposerAddOpen(false);
                                attachmentInputRef.current?.click();
                              }}
                            >
                              <Paperclip size={15} />
                              <span>
                                <strong>{t("composer.attachments")}</strong>
                                <small>{t("composer.attachmentsDesc")}</small>
                              </span>
                            </button>
                            <button
                              className={`composer-add-item ${
                                activePlan ? "selected" : ""
                              }`}
                              type="button"
                              role="menuitemcheckbox"
                              aria-checked={Boolean(activePlan)}
                              disabled={
                                !activeAgentScope || modeBusy || isStreaming
                              }
                              onClick={() => {
                                setComposerAddOpen(false);
                                void togglePlanMode();
                              }}
                            >
                              <ClipboardList size={15} />
                              <span>
                                <strong>{t("plan.label")}</strong>
                                <small>{t("plan.desc")}</small>
                              </span>
                              {activePlan && <Check size={14} />}
                            </button>
                          </div>

                          <div className="composer-add-divider" />
                          <div className="composer-add-heading">{t("skills.heading")}</div>
                          <div className="composer-skill-list">
                            {skillsBusy ? (
                              <div className="composer-add-empty">
                                <span className="spinner" />
                                {t("skills.loading")}
                              </div>
                            ) : skillsError ? (
                              <div className="composer-add-empty error">
                                {skillsError}
                                <button
                                  type="button"
                                  onClick={() => void loadAvailableSkills()}
                                >
                                  {t("common.retry")}
                                </button>
                              </div>
                            ) : availableSkills.length === 0 ? (
                              <div className="composer-add-empty">
                                {t("skills.empty")}
                              </div>
                            ) : (
                              availableSkills.map((skill) => {
                                const selected = promptSkills.some(
                                  (item) => item.name === skill.name,
                                );
                                return (
                                  <button
                                    className={`composer-add-item skill ${
                                      selected ? "selected" : ""
                                    }`}
                                    type="button"
                                    role="menuitemcheckbox"
                                    aria-checked={selected}
                                    key={skill.name}
                                    onClick={() => selectPromptSkill(skill)}
                                  >
                                    <Package size={15} />
                                    <span>
                                      <strong>{skill.name}</strong>
                                      <small>{skill.description}</small>
                                    </span>
                                    {selected && <Check size={14} />}
                                  </button>
                                );
                              })
                            )}
                          </div>
                        </div>
                      )}
                    </div>
                    <ToolbarSelect
                      className="model-select"
                      ariaLabel={t("model.select")}
                      icon={<Bot size={15} />}
                      value={selectedModel?.id ?? ""}
                      label={
                        modelsBusy
                          ? t("model.syncing")
                          : (selectedModel?.displayName ?? t("model.none"))
                      }
                      disabled={
                        modelsBusy ||
                        modelBusy ||
                        !activeAgentScope ||
                        !models.length
                      }
                      options={models.map((model) => ({
                        value: model.id,
                        label: model.displayName,
                        description: t("model.contextDesc", { size: formatContext(model.contextLength) }),
                      }))}
                      onChange={chooseModel}
                    />
                    {selectedModel?.supportsReasoning &&
                      supportedThinkingLevels.length > 0 && (
                        <ToolbarSelect
                          className="effort-select"
                          ariaLabel={t("thinking.select")}
                          icon={<BrainCircuit size={15} />}
                          value={effort}
                          label={t("thinking.label", { level: effort })}
                          disabled={modelBusy || !activeAgentScope}
                          options={supportedThinkingLevels.map((value) => ({
                            value,
                            label: t("thinking.label", { level: value }),
                            description: thinkingLevelDescription(value),
                          }))}
                          onChange={chooseEffort}
                        />
                      )}
                    <ToolbarSelect
                      className={`permission-select ${
                        permissionMode === "yolo"
                          ? "full-access"
                          : permissionMode === "auto"
                            ? "auto-access"
                            : ""
                      }`}
                      ariaLabel={t("permission.select")}
                      icon={<ShieldCheck size={15} />}
                      value={permissionMode}
                      label={
                        permissionMode === "yolo"
                          ? t("permission.yolo")
                          : permissionMode === "auto"
                            ? t("permission.auto")
                            : t("permission.manual")
                      }
                      disabled={modelBusy}
                      options={[
                        {
                          value: "manual",
                          label: t("permission.manual"),
                          description: t("permission.manualDesc"),
                        },
                        {
                          value: "auto",
                          label: t("permission.auto"),
                          description: t("permission.autoDesc"),
                        },
                        {
                          value: "yolo",
                          label: t("permission.yolo"),
                          description: t("permission.yoloDesc"),
                          danger: true,
                        },
                      ]}
                      onChange={(value) =>
                        choosePermissionMode(value as PermissionMode)
                      }
                    />
                    {selectedModel && (
                      <ContextUsageIndicator
                        usage={activeContextUsage}
                        agentUsage={activeAgentUsage}
                        models={models}
                        maxContextTokens={selectedModel.contextLength}
                      />
                    )}
                  </div>
                  <div className="send-zone">
                    <span>{isStreaming ? t("composer.enterQueue") : t("composer.enterSend")}</span>
                    <button
                      className="send-button"
                      type={showStopButton ? "button" : "submit"}
                      onClick={
                        showStopButton
                          ? () => void cancelActiveTurn()
                          : undefined
                      }
                      disabled={
                        showStopButton
                          ? !activeAgentScope
                          : hasBlockingInteraction ||
                            !composerHasContent ||
                            isHistoryLoading ||
                            modelBusy ||
                            !activeAgentScope ||
                            (promptAttachments.some(
                              (attachment) => attachment.kind === "image",
                            ) &&
                              !selectedModel?.supportsImage) ||
                            (promptAttachments.some(
                              (attachment) => attachment.kind === "video",
                            ) &&
                              !selectedModel?.supportsVideo)
                      }
                      title={showStopButton ? t("composer.stop") : isStreaming ? t("composer.queue") : t("composer.send")}
                    >
                      {showStopButton ? <X size={17} /> : <ArrowUp size={18} />}
                    </button>
                  </div>
                </div>
              </form>
              <p className="composer-caption">
                {t("composer.caption")}
              </p>
            </div>
          </>
        ) : (
          <ProjectLanding
            collapsed={sidebarCollapsed}
            menuButtonRef={mobileMenuButtonRef}
            onExpand={openSidebar}
            onAddProject={() => void addProject()}
          />
        )}
        </main>
        {sideChat ? (
          <SideChatSidebar
            state={sideChat}
            onDraftChange={updateSideChatDraft}
            onSend={() => void sendSideChatPrompt()}
            onClose={closeSideChat}
          />
        ) : compactionSummaryDetail ? (
          <CompactionSummarySidebar
            summary={compactionSummaryDetail}
            onClose={() => setCompactionSummaryDetail(undefined)}
          />
        ) : skillDetailTarget ? (
          <SkillDetailSidebar
            skill={skillDetail ?? skillDetailTarget}
            content={skillDetail?.content}
            path={skillDetail?.path}
            busy={skillDetailBusy}
            error={skillDetailError}
            onClose={closeSkillDetail}
            onRetry={() => void openSkillDetail(skillDetailTarget)}
          />
        ) : null}
      </div>

      {loginOpen && (
        <LoginDialog
          busy={loginBusy}
          code={deviceCode}
          onClose={() => !loginBusy && setLoginOpen(false)}
          onStart={() => void startLogin()}
        />
      )}

      {webAuthOpen && !isDesktop() && (
        <WebCredentialDialog
          onSubmit={(credential) => {
            setWebCredential(credential);
            setWebAuthOpen(false);
            window.location.reload();
          }}
        />
      )}

      {removalTarget && (
        <RemovalDialog
          target={removalTarget}
          busy={removalBusy}
          onClose={() => !removalBusy && setRemovalTarget(undefined)}
          onConfirm={() => void confirmRemoval()}
        />
      )}

      {directoryPickerOpen && !isDesktop() && (
        <DirectoryPickerDialog
          onClose={() => setDirectoryPickerOpen(false)}
          onSelect={(path) => {
            setDirectoryPickerOpen(false);
            void addProjectPath(path);
          }}
        />
      )}

      {settingsOpen && (
        <SettingsDialog
          appVersion={appVersion}
          colorScheme={colorScheme}
          language={language}
          onColorSchemeChange={updateColorScheme}
          onLanguageChange={updateLanguage}
          onClose={closeSettings}
        />
      )}

      {notice && (
        <div className="toast" role="status">
          <span>{notice}</span>
          <button aria-label={t("notice.dismiss")} onClick={() => setNotice(undefined)}>
            <X size={14} />
          </button>
        </div>
      )}
    </div>
  );
}

function AccountUsagePopover({
  appVersion,
  loggedIn,
  usage,
  busy,
  error,
  onRefresh,
  onLogin,
  onOpenSettings,
  onSignOut,
}: {
  appVersion?: string;
  loggedIn: boolean;
  usage?: AccountUsage;
  busy: boolean;
  error?: string;
  onRefresh: () => void;
  onLogin: () => void;
  onOpenSettings: () => void;
  onSignOut: () => void;
}) {
  const visibility = resolveAccountMenuVisibility(loggedIn);
  const rows = usage
    ? [...(usage.summary ? [usage.summary] : []), ...usage.limits]
    : [];

  return (
    <div
      id="account-popover"
      className="profile-popover"
      role="dialog"
      aria-label={
        visibility.showUsage
          ? t("account.usageTitle")
          : t("account.openMenu")
      }
    >
      <div className="profile-popover-header">
        <div className="profile-identity">
          <span className="profile-identity-mark">
            <Sparkles size={14} />
          </span>
          <span className="profile-identity-copy">
            <span className="profile-identity-title">
              <strong>Kimi Code</strong>
              {appVersion && <small>v{appVersion}</small>}
            </span>
          </span>
        </div>
        {visibility.showUsage && (
          <button
            className="profile-refresh"
            type="button"
            title={t("account.refreshUsage")}
            aria-label={t("account.refreshUsage")}
            disabled={busy}
            onClick={onRefresh}
          >
            <RefreshCw className={busy ? "spinning" : ""} size={13} />
          </button>
        )}
      </div>

      {visibility.showUsage && (
        <div className="account-usage-content" aria-live="polite">
          <div className="account-usage-heading">
            <span>{t("account.planUsage")}</span>
            {busy && usage && <small>{t("account.updating")}</small>}
          </div>

          {busy && !usage ? (
            <div className="account-usage-skeleton" aria-label={t("account.loadingUsage")}>
              <i />
              <i />
            </div>
          ) : rows.length > 0 ? (
            <div className="account-usage-list">
              {rows.map((row, index) => (
                <ManagedUsageProgress
                  key={`${row.label}-${String(index)}`}
                  row={row}
                  primary={index === 0 && usage?.summary !== null}
                />
              ))}
            </div>
          ) : (
            <div className="account-usage-empty">
              {error ? t("account.usageError") : t("account.noUsage")}
            </div>
          )}

          {error && (
            <div className="account-usage-error">
              <span>{error}</span>
              <button type="button" disabled={busy} onClick={onRefresh}>
                {t("common.retry")}
              </button>
            </div>
          )}

          {usage?.extraUsage && (
            <BoosterWalletSummary wallet={usage.extraUsage} />
          )}
        </div>
      )}

      <div className="profile-popover-footer">
        {visibility.showLogin && (
          <button className="profile-login" type="button" onClick={onLogin}>
            <LogIn size={14} />
            {t("account.login")}
          </button>
        )}
        <button
          className="profile-settings"
          type="button"
          onClick={onOpenSettings}
        >
          <SettingsIcon size={14} />
          {t("settings.title")}
        </button>
        {visibility.showSignOut && (
          <button className="profile-signout" type="button" onClick={onSignOut}>
            <LogOut size={14} />
            {t("account.signOut")}
          </button>
        )}
      </div>
    </div>
  );
}

function ManagedUsageProgress({
  row,
  primary,
}: {
  row: ManagedUsageRow;
  primary: boolean;
}) {
  const used = Math.max(0, row.used);
  const limit = Math.max(0, row.limit);
  const ratio = limit > 0 ? Math.min(1, used / limit) : 0;
  const percentage = Math.round(ratio * 100);
  const level = ratio >= 0.9 ? "danger" : ratio >= 0.72 ? "warning" : "";

  return (
    <div className={`managed-usage-row ${primary ? "primary" : ""}`}>
      <div className="managed-usage-label">
        <strong>{formatUsageLabel(row.label)}</strong>
        <span>{percentage}%</span>
      </div>
      <div
        className="managed-usage-track"
        role="progressbar"
        aria-label={row.label}
        aria-valuemin={0}
        aria-valuemax={limit}
        aria-valuenow={Math.min(used, limit)}
      >
        <i
          className={level}
          style={{ width: `${String(ratio * 100)}%` }}
        />
      </div>
      {row.resetHint && (
        <div className="managed-usage-meta">
          <span>{formatResetHint(row.resetHint)}</span>
        </div>
      )}
    </div>
  );
}

function BoosterWalletSummary({
  wallet,
}: {
  wallet: NonNullable<AccountUsage["extraUsage"]>;
}) {
  const hasMonthlyLimit =
    wallet.monthlyChargeLimitEnabled && wallet.monthlyChargeLimitCents > 0;
  const monthlyRatio = hasMonthlyLimit
    ? Math.min(1, wallet.monthlyUsedCents / wallet.monthlyChargeLimitCents)
    : 0;

  return (
    <div className="booster-wallet">
      <div className="account-usage-heading">
        <span>{t("account.extraUsage")}</span>
        <small>Booster</small>
      </div>
      <div className="booster-balance">
        <span>{t("account.balance")}</span>
        <strong>{formatCurrency(wallet.balanceCents, wallet.currency)}</strong>
      </div>
      <div className="booster-details">
        <span>
          {t("account.monthlyUsed", { amount: formatCurrency(wallet.monthlyUsedCents, wallet.currency) })}
        </span>
        <span>
          {hasMonthlyLimit
            ? t("account.monthlyLimit", { amount: formatCurrency(wallet.monthlyChargeLimitCents, wallet.currency) })
            : t("account.monthlyLimitUnlimited")}
        </span>
      </div>
      {hasMonthlyLimit && (
        <div className="managed-usage-track compact" aria-hidden="true">
          <i
            className={monthlyRatio >= 0.9 ? "danger" : monthlyRatio >= 0.72 ? "warning" : ""}
            style={{ width: `${String(monthlyRatio * 100)}%` }}
          />
        </div>
      )}
    </div>
  );
}

function formatUsageLabel(label: string): string {
  const normalized = label.trim().toLowerCase();
  if (normalized === "weekly limit") return t("usage.weeklyLimit");
  return label
    .replace(/^(\d+)h limit$/i, t("usage.hoursLimit", { count: "$1" }))
    .replace(/^(\d+)d limit$/i, t("usage.daysLimit", { count: "$1" }))
    .replace(/^(\d+)m limit$/i, t("usage.minutesLimit", { count: "$1" }));
}

function formatResetHint(hint: string): string {
  if (hint === "reset") return t("usage.resetDone");
  if (hint.startsWith("resets in "))
    return t("usage.resetsIn", { time: hint.slice(10) });
  if (hint.startsWith("resets at "))
    return t("usage.resetsAt", { time: hint.slice(10) });
  return hint;
}

function formatCurrency(cents: number, currency: string): string {
  try {
    return new Intl.NumberFormat(localeTag(), {
      style: "currency",
      currency: currency || "USD",
      currencyDisplay: "narrowSymbol",
    }).format(cents / 100);
  } catch {
    return `${(cents / 100).toFixed(2)} ${currency}`;
  }
}

interface ToolbarSelectOption {
  value: string;
  label: string;
  description?: string;
  danger?: boolean;
}

function ChatHeaderTitle({
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

function WindowTitleBar() {
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

function ContextUsageIndicator({
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

function TodoProgress({ todos }: { todos: readonly TodoItem[] }) {
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

function BackgroundTaskProgress({
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

function compactionTokenTransition(
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

function CompactionNotice({ event }: { event: CompactionEvent }) {
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

function ToolbarSelect({
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

function isPlanReviewInteraction(interaction: AgentInteraction): boolean {
  const payload = interaction.payload as Partial<ApprovalPayload>;
  return payload.display?.kind === "plan_review";
}

function QuestionCard({
  interaction,
  busy,
  onRespond,
}: {
  interaction: AgentInteraction;
  busy: boolean;
  onRespond: (response: QuestionResponse | null) => void;
}) {
  const payload = interaction.payload as QuestionPayload;
  const questions = Array.isArray(payload.questions) ? payload.questions : [];
  const [selections, setSelections] = useState<Record<number, string[]>>({});
  const [otherAnswers, setOtherAnswers] = useState<Record<number, string>>({});

  const toggleOption = (
    questionIndex: number,
    label: string,
    multiSelect: boolean,
  ): void => {
    setSelections((current) => {
      const selected = current[questionIndex] ?? [];
      const next = multiSelect
        ? selected.includes(label)
          ? selected.filter((value) => value !== label)
          : [...selected, label]
        : [label];
      return { ...current, [questionIndex]: next };
    });
    if (!multiSelect) {
      setOtherAnswers((current) => ({ ...current, [questionIndex]: "" }));
    }
  };

  const updateOtherAnswer = (
    questionIndex: number,
    value: string,
    multiSelect: boolean,
  ): void => {
    setOtherAnswers((current) => ({ ...current, [questionIndex]: value }));
    if (!multiSelect && value.trim()) {
      setSelections((current) => ({ ...current, [questionIndex]: [] }));
    }
  };

  const answers = questions.map((question, questionIndex) => {
    const selected = selections[questionIndex] ?? [];
    const other = otherAnswers[questionIndex]?.trim();
    return other ? [...selected, other] : selected;
  });
  const canSubmit =
    questions.length > 0 && answers.every((answer) => answer.length > 0);

  const submit = (): void => {
    if (!canSubmit || busy) return;
    const responseAnswers: Record<string, string> = {};
    questions.forEach((question, questionIndex) => {
      responseAnswers[question.question] = answers[questionIndex]?.join(", ") ?? "";
    });
    onRespond({ answers: responseAnswers, method: "enter" });
  };

  return (
    <section className="interaction-card question-card" aria-live="polite">
      <div className="interaction-card-heading">
        <span className="interaction-card-icon">
          <MessageSquareText size={18} />
        </span>
        <div>
          <small>{t("question.subtitle")}</small>
          <strong>{t("question.title")}</strong>
        </div>
      </div>
      <div className="question-list">
        {questions.map((question, questionIndex) => {
          const selected = selections[questionIndex] ?? [];
          const multiSelect = question.multiSelect === true;
          return (
            <fieldset className="question-item" key={`${question.question}-${questionIndex}`}>
              <legend>
                {question.header && <span>{question.header}</span>}
                <strong>{question.question}</strong>
                {question.body && <small>{question.body}</small>}
              </legend>
              <div className="question-options">
                {question.options.map((option) => {
                  const checked = selected.includes(option.label);
                  return (
                    <button
                      type="button"
                      className={checked ? "selected" : ""}
                      key={option.label}
                      disabled={busy}
                      aria-pressed={checked}
                      onClick={() =>
                        toggleOption(questionIndex, option.label, multiSelect)
                      }
                    >
                      <span className={multiSelect ? "option-check" : "option-radio"}>
                        {checked && <Check size={12} />}
                      </span>
                      <span>
                        <strong>{option.label}</strong>
                        {option.description && <small>{option.description}</small>}
                      </span>
                    </button>
                  );
                })}
              </div>
              <label className="question-other">
                <span>{question.otherLabel || t("question.other")}</span>
                <input
                  value={otherAnswers[questionIndex] ?? ""}
                  disabled={busy}
                  placeholder={question.otherDescription || t("question.otherPlaceholder")}
                  onChange={(event) =>
                    updateOtherAnswer(questionIndex, event.target.value, multiSelect)
                  }
                />
              </label>
            </fieldset>
          );
        })}
      </div>
      <div className="interaction-card-actions">
        <button
          type="button"
          className="interaction-secondary"
          disabled={busy}
          onClick={() => onRespond(null)}
        >
          {t("question.skip")}
        </button>
        <button
          type="button"
          className="interaction-primary"
          disabled={busy || !canSubmit}
          onClick={submit}
        >
          {busy ? <span className="spinner light" /> : <Check size={14} />}
          {t("question.submit")}
        </button>
      </div>
    </section>
  );
}

function PlanReviewCard({
  interaction,
  busy,
  onRespond,
}: {
  interaction: AgentInteraction;
  busy: boolean;
  onRespond: (response: Record<string, unknown>) => void;
}) {
  const payload = interaction.payload as ApprovalPayload;
  const display = payload.display as PlanReviewDisplay;
  const [feedback, setFeedback] = useState("");
  const options =
    display.options && display.options.length >= 2 ? display.options : [];
  const [selectedLabel, setSelectedLabel] = useState<string | undefined>(
    () => options[0]?.label,
  );
  const trimmedFeedback = feedback.trim();
  const needsRevision = trimmedFeedback.length > 0;
  const canExecute = options.length === 0 || selectedLabel !== undefined;

  const submitReview = (): void => {
    if (needsRevision) {
      onRespond({
        decision: "rejected",
        selectedLabel: "Revise",
        feedback: trimmedFeedback,
      });
      return;
    }
    onRespond({
      decision: "approved",
      selectedLabel: selectedLabel ?? "Approve",
    });
  };

  return (
    <section className="interaction-card plan-review-card" aria-live="polite">
      <div className="interaction-card-heading">
        <span className="interaction-card-icon">
          <ClipboardList size={18} />
        </span>
        <div>
          <small>{t("plan.completed")}</small>
          <strong>{t("plan.reviewTitle")}</strong>
        </div>
      </div>
      <div className="plan-review-content">
        <MarkdownMessage content={display.plan} />
      </div>
      {display.path && <code className="plan-review-path">{display.path}</code>}
      {options.length > 0 && (
        <div className="plan-review-options">
          <span>{t("plan.chooseOption")}</span>
          <div className="plan-review-option-list" role="radiogroup">
            {options.map((option) => (
              <label
                className={`plan-review-option ${
                  selectedLabel === option.label ? "selected" : ""
                } ${busy ? "disabled" : ""}`}
                key={option.label}
              >
                <input
                  type="radio"
                  name={`plan-review-${interaction.id}`}
                  value={option.label}
                  checked={selectedLabel === option.label}
                  disabled={busy}
                  onChange={() => setSelectedLabel(option.label)}
                />
                <span className="plan-review-option-copy">
                  <strong>{option.label}</strong>
                  {option.description && <small>{option.description}</small>}
                </span>
              </label>
            ))}
          </div>
        </div>
      )}
      <label className="plan-review-feedback">
        <span>{t("plan.feedbackLabel")}</span>
        <textarea
          rows={2}
          value={feedback}
          disabled={busy}
          placeholder={t("plan.feedbackPlaceholder")}
          onChange={(event) => setFeedback(event.target.value)}
        />
      </label>
      <div className="interaction-card-actions plan-review-actions">
        <button
          type="button"
          className="interaction-danger"
          disabled={busy}
          onClick={() =>
            onRespond({ decision: "rejected", selectedLabel: "Reject" })
          }
        >
          {t("common.reject")}
        </button>
        <button
          type="button"
          className={
            needsRevision ? "interaction-secondary" : "interaction-primary"
          }
          disabled={busy || (!needsRevision && !canExecute)}
          onClick={submitReview}
        >
          {busy ? (
            <span className="spinner light" />
          ) : needsRevision ? (
            <RefreshCw size={14} />
          ) : (
            <Check size={14} />
          )}
          {needsRevision ? t("plan.revise") : t("plan.execute")}
        </button>
      </div>
    </section>
  );
}

function ApprovalCard({
  interaction,
  busy,
  onReject,
  onApprove,
  onApproveSession,
}: {
  interaction: AgentInteraction;
  busy: boolean;
  onReject: () => void;
  onApprove: () => void;
  onApproveSession: () => void;
}) {
  const payload = interaction.payload as ApprovalPayload;
  const display = payload.display;
  const isCommand = display?.kind === "command" && "command" in display;
  const command = isCommand ? String(display.command) : undefined;
  const cwd = isCommand && display.cwd ? String(display.cwd) : undefined;
  const detail =
    !isCommand && display
      ? ("path" in display && display.path) ||
        ("summary" in display && display.summary) ||
        payload.action
      : undefined;

  return (
    <section className="approval-card" aria-live="polite">
      <div className="approval-icon">
        <ShieldAlert size={19} />
      </div>
      <div className="approval-content">
        <div className="approval-heading">
          <div>
            <span>{t("approval.title")}</span>
            <strong>{payload.action || t("approval.toolRequest", { tool: payload.toolName })}</strong>
          </div>
          <span className="approval-tool">{payload.toolName}</span>
        </div>
        {command ? (
          <div className="approval-command">
            <div>
              <TerminalSquare size={13} />
              <span>{cwd || t("approval.currentDir")}</span>
            </div>
            <code>{command}</code>
          </div>
        ) : (
          <div className="approval-detail">{String(detail || t("approval.needsConfirm"))}</div>
        )}
        <div className="approval-footer">
          <p>{t("approval.warning")}</p>
          <div className="approval-actions">
            <button type="button" className="approval-reject" onClick={onReject} disabled={busy}>
              {t("common.reject")}
            </button>
            <button type="button" className="approval-session" onClick={onApproveSession} disabled={busy}>
              {t("approval.allowSession")}
            </button>
            <button type="button" className="approval-once" onClick={onApprove} disabled={busy}>
              {busy ? <span className="spinner light" /> : <Check size={14} />}
              {t("approval.allowOnce")}
            </button>
          </div>
        </div>
      </div>
    </section>
  );
}

const ConversationOutline = memo(function ConversationOutline({
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

function Welcome({
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
        {PROMPT_SUGGESTIONS.map((suggestion) => (
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

function LiveTurnView({
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
          <AssistantResponseStatus
            running={isTurnRunning(turn)}
            durationMs={turn.durationMs}
          />
        </div>
      </article>
    </section>
  );
}

function LiveSteeredPromptView({
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

function QueuedPromptList({
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
                  prompt.skills.length > 0
                }
                title={
                  prompt.skills.length > 0
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

function RemoteQueuedPromptList({
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

function AssistantResponseStatus({
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

function Collapsible({
  open,
  className = "",
  children,
}: {
  open: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      className={`collapsible ${open ? "open" : ""} ${className}`.trim()}
      aria-hidden={!open}
    >
      <div className="collapsible-inner">{children}</div>
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

function LiveThinkingBlock({ content }: { content: string }) {
  return <ThinkingSummary content={content} />;
}

function LiveAssistantContent({
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

function LiveToolBlock({
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
            <section className="tool-detail-section">
              <span>{t("tool.result")}</span>
              <pre className={tool.isError ? "error" : ""}>
                {structuredValue(tool.output)}
              </pre>
            </section>
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

const HistoryTurnView = memo(function HistoryTurnView({
  turn,
  toolResults,
  subagentRuns,
  subagentLiveTurns,
  messageDurations,
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

function UserMessageView({
  message,
  toolResults,
  subagentRuns,
  subagentLiveTurns,
  onSkillOpen,
}: {
  message: RenderMessage;
  toolResults: Map<string, ToolResultContent>;
  subagentRuns?: SubagentRunsByTool;
  subagentLiveTurns?: Record<string, InFlightTurn>;
  onSkillOpen: (name: string) => void;
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

function messageText(message: ProtocolMessage): string {
  return message.content
    .filter(
      (part): part is Extract<MessageContent, { type: "text" }> =>
        part.type === "text" && embeddedMediaContent(part.text) === undefined,
    )
    .map((part) => part.text)
    .join("");
}

function displayMessageText(message: ProtocolMessage): string {
  return parseSkillPromptDisplay(messageText(message)).text;
}

function embeddedMediaContent(text: string): MessageContent | undefined {
  for (const type of ["audio", "video"] as const) {
    const prefix = `[${type}:`;
    if (text.startsWith(prefix) && text.endsWith("]")) {
      const url = text.slice(prefix.length, -1);
      if (url) return { type, source: { kind: "url", url } };
    }
  }
  return undefined;
}

function messageStructuredContent(message: ProtocolMessage): MessageContent[] {
  return message.content.flatMap((part) => {
    if (part.type === "thinking") return [];
    if (part.type !== "text") return [part];
    const media = embeddedMediaContent(part.text);
    return media ? [media] : [];
  });
}

function messageThinking(message: ProtocolMessage): string {
  return message.content
    .filter(
      (part): part is Extract<MessageContent, { type: "thinking" }> =>
        part.type === "thinking",
    )
    .map((part) => part.thinking)
    .join("");
}

function structuredValue(value: unknown): string {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function mediaSourceUrl(
  source: Extract<
    MessageContent,
    { type: "image" | "audio" | "video" }
  >["source"],
): string | undefined {
  if (source.kind === "url") return source.url;
  if (source.kind === "base64") {
    return `data:${source.media_type};base64,${source.data}`;
  }
  return undefined;
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

function PromptAttachmentContent({
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

function ToolInputView({
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
  return <pre>{structuredValue(input)}</pre>;
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
            <section className="tool-detail-section">
              <span>{t("tool.result")}</span>
              <pre className={result.is_error ? "error" : ""}>
                {structuredValue(result.output)}
              </pre>
            </section>
          )}
        </div>
      </Collapsible>
    </div>
  );
}

function StructuredMessageContent({
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

function SideChatSidebar({
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

function CompactionSummarySidebar({
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

function SkillDetailSidebar({
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

function MarkdownCodeBlock({ children }: { children: ReactNode }) {
  const className = isValidElement<{ className?: string }>(children)
    ? children.props.className
    : undefined;
  const language = className?.match(/language-([^\s]+)/)?.[1] ?? "code";

  return (
    <div className="code-wrap">
      <div className="code-label">
        <span>{language}</span>
      </div>
      <pre>{children}</pre>
    </div>
  );
}

const MarkdownMessage = memo(function MarkdownMessage({
  content,
}: {
  content: string;
}) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm]}
      components={{
        pre({ children }) {
          return <MarkdownCodeBlock>{children}</MarkdownCodeBlock>;
        },
        code({ className, children, ...props }) {
          return (
            <code className={className} {...props}>
              {children}
            </code>
          );
        },
        table({ children }) {
          return (
            <div className="markdown-table-wrap">
              <table>{children}</table>
            </div>
          );
        },
        a({ children, href, ...props }) {
          const externalUrl = resolveMarkdownExternalUrl(href);
          return (
            <a
              {...props}
              href={externalUrl}
              target="_blank"
              rel="noopener noreferrer"
              onClick={(event) => {
                if (!externalUrl) {
                  event.preventDefault();
                  return;
                }
                event.preventDefault();
                void openExternalUrl(externalUrl).catch((error) => {
                  console.error("failed to open Markdown link", error);
                });
              }}
            >
              {children}
            </a>
          );
        },
      }}
    >
      {content}
    </ReactMarkdown>
  );
});

function StreamingMarkdownMessage({
  content,
  active,
}: {
  content: string;
  active: boolean;
}) {
  const latestContent = useRef(content);
  latestContent.current = content;
  const [displayedContent, setDisplayedContent] = useState(content);

  useLayoutEffect(() => {
    if (!active) setDisplayedContent(content);
  }, [active, content]);

  useEffect(() => {
    if (!active) return;
    const interval = window.setInterval(() => {
      setDisplayedContent(latestContent.current);
    }, 80);
    return () => window.clearInterval(interval);
  }, [active]);

  return <MarkdownMessage content={displayedContent} />;
}

function ProjectLanding({
  collapsed,
  menuButtonRef,
  onExpand,
  onAddProject,
}: {
  collapsed: boolean;
  menuButtonRef?: React.RefObject<HTMLButtonElement | null>;
  onExpand: () => void;
  onAddProject: () => void;
}) {
  return (
    <div className="project-landing">
      {collapsed && (
        <button
          className="landing-menu icon-button"
          ref={menuButtonRef}
          type="button"
          aria-label={t("sidebar.expand")}
          onClick={onExpand}
        >
          <Menu size={18} />
        </button>
      )}
      <div className="landing-visual">
        <span className="landing-grid" />
        <div className="landing-folder">
          <FolderGit2 size={42} />
        </div>
        <i className="landing-dot dot-one" />
        <i className="landing-dot dot-two" />
        <i className="landing-dot dot-three" />
      </div>
      <p className="eyebrow">YOUR AI CODING PARTNER</p>
      <h1>{t("landing.title")}</h1>
      <p>
        {t("landing.copy1")}
        <br />
        {t("landing.copy2")}
      </p>
      <button className="landing-primary" onClick={onAddProject}>
        <Folder size={17} />
        {t("landing.openProject")}
      </button>
      <div className="landing-shortcut">
        <span>{t("landing.tip")}</span>
        {t("landing.dragHint")}
      </div>
    </div>
  );
}

function RemovalDialog({
  target,
  busy,
  onClose,
  onConfirm,
}: {
  target: RemovalTarget;
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const isProject = target.kind === "project";

  useEffect(() => {
    const closeOnEscape = (event: globalThis.KeyboardEvent): void => {
      if (event.key === "Escape" && !busy) onClose();
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [busy, onClose]);

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <section
        className="operation-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="removal-dialog-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <button
          className="dialog-close"
          type="button"
          aria-label={t("removal.close")}
          onClick={onClose}
          disabled={busy}
        >
          <X size={17} />
        </button>
        <div
          className={`operation-dialog-icon ${
            isProject ? "project" : "conversation"
          }`}
        >
          {isProject ? <FolderMinus size={23} /> : <Archive size={22} />}
        </div>
        <p className="eyebrow">
          {isProject ? "WORKSPACE CATALOG" : "CONVERSATION ARCHIVE"}
        </p>
        <h2 id="removal-dialog-title">
          {isProject ? t("removal.projectTitle") : t("removal.conversationTitle")}
        </h2>
        <p className="dialog-copy">
          {isProject
            ? t("removal.projectCopy", { name: target.name })
            : t("removal.conversationCopy", { title: target.title })}
        </p>
        {isProject && <div className="operation-target">{target.path}</div>}
        <div className="operation-dialog-actions">
          <button
            className="dialog-secondary"
            type="button"
            onClick={onClose}
            disabled={busy}
            autoFocus
          >
            {t("common.cancel")}
          </button>
          <button
            className="dialog-danger"
            type="button"
            onClick={onConfirm}
            disabled={busy}
          >
            {busy ? (
              <>
                <span className="spinner light" />
                {t("common.processing")}
              </>
            ) : isProject ? (
              <>
                <FolderMinus size={15} />
                {t("sidebar.removeProject")}
              </>
            ) : (
              <>
                <Archive size={15} />
                {t("conversation.archive")}
              </>
            )}
          </button>
        </div>
      </section>
    </div>
  );
}

function DirectoryPickerDialog({
  onClose,
  onSelect,
}: {
  onClose: () => void;
  onSelect: (path: string) => void;
}) {
  const [home, setHome] = useState<FolderHome>();
  const [browse, setBrowse] = useState<FolderBrowse>();
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<string>();

  const navigate = useCallback(async (target?: string): Promise<void> => {
    setBusy(true);
    setError(undefined);
    try {
      const result = await invoke<FolderBrowse>("fs_browse", {
        ...(target ? { path: target } : {}),
      });
      setBrowse(result);
      setPath(result.path);
    } catch (cause) {
      setError(conciseError(cause));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    let active = true;
    void invoke<FolderHome>("fs_home")
      .then((result) => {
        if (!active) return;
        setHome(result);
        return navigate(result.home);
      })
      .catch((cause) => {
        if (active) {
          setError(conciseError(cause));
          setBusy(false);
        }
      });
    return () => {
      active = false;
    };
  }, [navigate]);

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <section
        className="directory-picker-dialog"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <button className="dialog-close" aria-label={t("login.close")} onClick={onClose}>
          <X size={17} />
        </button>
        <p className="eyebrow">KIMI CODE WEB</p>
        <h2>{t("folderPicker.title")}</h2>
        <form
          className="directory-path-form"
          onSubmit={(event) => {
            event.preventDefault();
            void navigate(path.trim());
          }}
        >
          <input
            value={path}
            onChange={(event) => setPath(event.target.value)}
            aria-label={t("folderPicker.path")}
          />
          <button type="submit" className="dialog-secondary" disabled={!path.trim() || busy}>
            {t("folderPicker.go")}
          </button>
        </form>
        {home && (
          <div className="directory-roots">
            {[home.home, ...home.recent_roots]
              .filter((item, index, values) => values.indexOf(item) === index)
              .map((root) => (
                <button type="button" key={root} onClick={() => void navigate(root)}>
                  <Folder size={13} />
                  <span>{root}</span>
                </button>
              ))}
          </div>
        )}
        <div className="directory-list">
          {busy ? (
            <div className="history-loading"><span className="spinner" />{t("folderPicker.loading")}</div>
          ) : error ? (
            <div className="history-loading error">{error}</div>
          ) : (
            <>
              {browse?.parent && (
                <button type="button" onClick={() => void navigate(browse.parent ?? undefined)}>
                  <Folder size={16} />
                  <span>..</span>
                  <ChevronRight size={15} />
                </button>
              )}
              {browse?.entries.map((entry) => (
                <button type="button" key={entry.path} onClick={() => void navigate(entry.path)}>
                  <Folder size={16} />
                  <span>{entry.name}</span>
                  <ChevronRight size={15} />
                </button>
              ))}
            </>
          )}
        </div>
        <div className="operation-dialog-actions">
          <button className="dialog-secondary" type="button" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            className="dialog-primary"
            type="button"
            disabled={!browse?.path || busy}
            onClick={() => browse?.path && onSelect(browse.path)}
          >
            {t("folderPicker.select")}
          </button>
        </div>
      </section>
    </div>
  );
}

function LoginDialog({
  busy,
  code,
  onClose,
  onStart,
}: {
  busy: boolean;
  code?: DeviceCode;
  onClose: () => void;
  onStart: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const copyCode = async (): Promise<void> => {
    if (!code) return;
    await navigator.clipboard.writeText(code.userCode);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };
  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <section className="login-dialog" onMouseDown={(event) => event.stopPropagation()}>
        <button
          className="dialog-close"
          aria-label={t("login.close")}
          onClick={onClose}
          disabled={busy}
        >
          <X size={17} />
        </button>
        <div className="login-logo">
          <Sparkles size={24} />
        </div>
        <p className="eyebrow">KIMI CODE ACCOUNT</p>
        <h2>{t("login.title")}</h2>
        <p className="dialog-copy">
          {t("login.copy")}
        </p>
        {code ? (
          <>
            <button className="device-code" onClick={() => void copyCode()}>
              <span>{t("login.deviceCode")}</span>
              <strong>{code.userCode}</strong>
              <small>{copied ? t("common.copied") : t("login.clickToCopy")}</small>
            </button>
            <button
              className="dialog-primary"
              onClick={() =>
                void openExternalUrl(
                  code.verificationUriComplete || code.verificationUri,
                )
              }
            >
              {t("login.authorize")}
              <ExternalLink size={16} />
            </button>
            <div className="waiting-line">
              <span className="spinner" />
              {t("login.waiting")}
            </div>
          </>
        ) : (
          <>
            <div className="login-features">
              <span><Check size={14} /> {t("login.featureOauth")}</span>
              <span><Check size={14} /> {t("login.featureSync")}</span>
              <span><Check size={14} /> {t("login.featureLocal")}</span>
            </div>
            <button className="dialog-primary" onClick={onStart} disabled={busy}>
              {busy ? (
                <>
                  <span className="spinner light" />
                  {t("login.creating")}
                </>
              ) : (
                <>
                  {t("login.continue")}
                  <ArrowUp size={16} />
                </>
              )}
            </button>
          </>
        )}
      </section>
    </div>
  );
}

function WebCredentialDialog({
  onSubmit,
}: {
  onSubmit: (credential: string) => void;
}) {
  const [credential, setCredential] = useState("");
  const submit = (event: FormEvent): void => {
    event.preventDefault();
    const value = credential.trim();
    if (value) onSubmit(value);
  };
  return (
    <div className="modal-backdrop">
      <form
        className="login-dialog"
        onSubmit={submit}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="login-logo">
          <ShieldCheck size={24} />
        </div>
        <p className="eyebrow">KIMI CODE WEB</p>
        <h2>{t("webAuth.title")}</h2>
        <p className="dialog-copy">{t("webAuth.description")}</p>
        <input
          className="web-credential-input"
          type="password"
          autoFocus
          autoComplete="off"
          value={credential}
          placeholder={t("webAuth.placeholder")}
          onChange={(event) => setCredential(event.target.value)}
        />
        <button className="dialog-primary" type="submit" disabled={!credential.trim()}>
          {t("webAuth.connect")}
          <ArrowUp size={16} />
        </button>
      </form>
    </div>
  );
}
