import {
  type FormEvent,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  isValidElement,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
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
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
  Square,
  SquarePen,
  TerminalSquare,
  Trash2,
  Wrench,
  X,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import {
  archiveSession,
  createAgentClient,
  createOrTouchWorkspace,
  listWorkspaceSessions,
  prepareSession,
  removeWorkspace,
  setDefaultModel,
  subscribeAgentEvents,
  unsubscribeAgentEvents,
} from "./agentRpc";
import {
  conversationFromSession,
  getActive,
  loadDesktopState,
  projectFromWorkspace,
} from "./store";
import type {
  AccountUsage,
  AgentChatEvent,
  AgentChatEventEnvelope,
  AgentContentPart,
  AgentInteraction,
  AgentInteractionsEvent,
  ApprovalPayload,
  AuthStatus,
  CompactionEvent,
  ContextUsage,
  DesktopState,
  DeviceCode,
  MessageContent,
  MessagePage,
  ManagedUsageRow,
  Model,
  PermissionMode,
  PlanData,
  PlanReviewDisplay,
  Project,
  ProtocolMessage,
  QuestionPayload,
  QuestionResponse,
  ToolUpdate,
} from "./types";

const HISTORY_PAGE_SIZE = 50;

type RenderMessage = ProtocolMessage & {
  status?: "streaming" | "done" | "error";
};

interface ConversationHistory {
  conversationId: string;
  items: ProtocolMessage[];
  hasMore: boolean;
  loading: boolean;
  loadingMore: boolean;
  error?: string;
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
  prompt: string;
  createdAt: string;
  turnId?: number;
  status: LiveTurnStatus;
  durationMs?: number;
  steps: LiveStep[];
  error?: string;
  historyBoundaryId?: string;
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

const PROMPT_SUGGESTIONS = [
  {
    icon: <FileCode2 size={17} />,
    title: "理解这个项目",
    prompt: "分析这个项目的结构、核心模块和运行方式，给我一份简洁的导览。",
  },
  {
    icon: <Wrench size={17} />,
    title: "排查一个问题",
    prompt: "帮我检查这个项目中潜在的错误和可维护性问题，按优先级给出建议。",
  },
  {
    icon: <TerminalSquare size={17} />,
    title: "开始一个功能",
    prompt: "先阅读项目结构，然后帮我规划并实现一个新功能。",
  },
];

function formatTime(timestamp: string | number): string {
  return new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(timestamp));
}

function formatContext(value: number): string {
  if (value >= 1_000_000) return `${Math.round(value / 1_000_000)}M`;
  if (value >= 1_000) return `${Math.round(value / 1_000)}K`;
  return `${value}`;
}

function formatTokenCount(value: number): string {
  return Math.max(0, Math.round(value)).toLocaleString("en-US");
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

function fetchMessagePage(
  conversationId: string,
  beforeId?: string,
): Promise<MessagePage> {
  return invoke<MessagePage>("list_conversation_messages", {
    sessionId: conversationId,
    beforeId,
    pageSize: HISTORY_PAGE_SIZE,
  });
}

function newInFlightTurn(
  prompt: string,
  historyBoundaryId?: string,
): InFlightTurn {
  return {
    prompt,
    createdAt: new Date().toISOString(),
    status: "queued",
    steps: [],
    historyBoundaryId,
  };
}

function isTurnRunning(turn?: InFlightTurn): boolean {
  return turn?.status === "queued" || turn?.status === "running";
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
    if (last?.kind === kind) {
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
  const steps = turn.steps.map((step) => ({
    ...step,
    blocks: [...step.blocks],
  }));
  for (let stepIndex = steps.length - 1; stepIndex >= 0; stepIndex -= 1) {
    const blockIndex = steps[stepIndex].blocks.findIndex(
      (block) => block.kind === "tool" && block.toolCallId === toolCallId,
    );
    if (blockIndex >= 0) {
      const block = steps[stepIndex].blocks[blockIndex];
      if (block.kind === "tool") {
        steps[stepIndex].blocks[blockIndex] = update(block);
      }
      return { ...turn, steps };
    }
  }

  return withCurrentStep({ ...turn, steps }, (step) => ({
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
  if (turn.turnId !== undefined && turn.turnId !== event.turnId) return turn;
  const next =
    turn.turnId === undefined ? { ...turn, turnId: event.turnId } : turn;

  switch (event.type) {
    case "turn.started":
      return { ...next, status: "running" };
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
      const index = next.steps.findIndex(
        (step) =>
          (event.stepId && step.stepId === event.stepId) ||
          step.step === event.step,
      );
      if (index < 0) {
        return {
          ...next,
          status: "running",
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
      return { ...next, status: "running", steps };
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
        updates: [...tool.updates, event.update],
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

const CHAT_EVENT_TYPES = new Set([
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
  if (turn.historyBoundaryId) {
    const boundary = items.findIndex(
      (message) => message.id === turn.historyBoundaryId,
    );
    if (boundary >= 0) return items.slice(0, boundary + 1);
  }

  const prompt = turn.prompt;
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const message = items[index];
    if (message.role === "user" && messageText(message) === prompt) {
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
  if (turn.historyBoundaryId) {
    const boundary = items.findIndex(
      (message) => message.id === turn.historyBoundaryId,
    );
    if (boundary >= 0) startIndex = boundary + 1;
  } else {
    for (let index = items.length - 1; index >= 0; index -= 1) {
      const message = items[index];
      if (message.role === "user" && messageText(message) === turn.prompt) {
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
  if (totalSeconds < 10) return `${totalSeconds.toFixed(1)} 秒`;
  const roundedSeconds = Math.round(totalSeconds);
  if (roundedSeconds < 60) return `${roundedSeconds} 秒`;
  const minutes = Math.floor(roundedSeconds / 60);
  const seconds = roundedSeconds % 60;
  return seconds > 0 ? `${minutes} 分 ${seconds} 秒` : `${minutes} 分钟`;
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
  const [desktop, setDesktop] = useState<DesktopState>({ projects: [] });
  const [auth, setAuth] = useState<AuthStatus>({
    loggedIn: false,
    provider: "kimi-code",
  });
  const [models, setModels] = useState<Model[]>([]);
  const [prompt, setPrompt] = useState("");
  const [effort, setEffort] = useState("medium");
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [loginOpen, setLoginOpen] = useState(false);
  const [loginBusy, setLoginBusy] = useState(false);
  const [deviceCode, setDeviceCode] = useState<DeviceCode>();
  const [profileOpen, setProfileOpen] = useState(false);
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
  const [contextUsages, setContextUsages] = useState<
    Record<string, ContextUsage>
  >({});
  const [messageDurations, setMessageDurations] = useState<
    Record<string, Record<string, number>>
  >({});
  const [plans, setPlans] = useState<Record<string, PlanData | null>>({});
  const [modeBusy, setModeBusy] = useState(false);
  const [removalTarget, setRemovalTarget] = useState<RemovalTarget>();
  const [removalBusy, setRemovalBusy] = useState(false);
  const [history, setHistory] = useState<ConversationHistory>();
  const [inFlightTurns, setInFlightTurns] = useState<
    Record<string, InFlightTurn>
  >({});
  const [activeAgentScope, setActiveAgentScope] = useState<{
    sessionId: string;
    agentId: string;
  }>();
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const profileRef = useRef<HTMLDivElement>(null);
  const noticeTimer = useRef<number | undefined>(undefined);
  const accountUsageRequest = useRef(0);
  const historyRequests = useRef<Record<string, number>>({});
  const agentSubscriptions = useRef<Map<string, AgentSubscription>>(new Map());
  const pendingAgentSubscriptions = useRef<
    Map<string, PendingAgentSubscription>
  >(new Map());

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
  const permissionMode = activeConversation?.permissionMode ?? "manual";
  const activeTurn = activeConversation
    ? inFlightTurns[activeConversation.id]
    : undefined;
  const activeHistory =
    history?.conversationId === activeConversation?.id ? history : undefined;
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
      activeTurn?.prompt,
    ],
  );
  const historyToolPresentation = useMemo(
    () => mergeHistoryToolResults(visibleHistoryMessages),
    [visibleHistoryMessages],
  );
  const hasVisibleMessages =
    historyToolPresentation.messages.length > 0 || activeTurn !== undefined;
  const isStreaming = isTurnRunning(activeTurn);
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
  const activePlan = activeConversation
    ? plans[activeConversation.id]
    : undefined;

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

  const refreshModeState = async (scope: {
    sessionId: string;
    agentId: string;
  }): Promise<void> => {
    const agent = createAgentClient(scope);
    const plan = await agent.getPlan();
    setPlans((current) => ({ ...current, [scope.sessionId]: plan }));
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

  const loadModels = async (refresh = false): Promise<void> => {
    setModelsBusy(true);
    try {
      const nextModels = await invoke<Model[]>("list_models", { refresh });
      setModels(nextModels);
      if (nextModels.length === 0) showNotice("当前账号没有可用模型");
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setModelsBusy(false);
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
    if (opening) void loadAccountUsage();
  };

  useEffect(() => {
    let active = true;
    loadDesktopState()
      .then((state) => {
        if (active) setDesktop(state);
      })
      .catch(() => {
        // Vite's browser preview has no Tauri bridge.
      });
    invoke<AuthStatus>("auth_status")
      .then((status) => {
        if (!active) return;
        setAuth(status);
        if (status.loggedIn) void loadModels();
      })
      .catch(() => {
        // Vite's browser preview has no Tauri bridge; the actual desktop app does.
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    void getVersion()
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
    setActiveAgentScope(undefined);
    if (
      !auth.loggedIn ||
      !activeProject ||
      !activeConversation ||
      !selectedModel
    ) {
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
        updateDesktop((current) => ({
          ...current,
          projects: current.projects.map((project) =>
            project.id !== activeProject.id
              ? project
              : {
                  ...project,
                  conversations: project.conversations.map((conversation) =>
                    conversation.id === activeConversation.id
                      ? { ...conversation, modelId: scope.model }
                      : conversation,
                  ),
                },
          ),
        }));
        setActiveAgentScope(scope);
        await refreshModeState(scope);
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
    auth.loggedIn,
    models.length,
  ]);

  useEffect(
    () => () => releaseAllAgentSubscriptions(),
    [],
  );

  useEffect(() => {
    const unlistenDevice = listen<DeviceCode>("auth-device-code", (event) => {
      setDeviceCode(event.payload);
      setLoginOpen(true);
    });
    const unlistenBrowserError = listen<string>(
      "auth-browser-open-failed",
      (event) => {
        showNotice(`未能自动打开浏览器：${event.payload}`);
      },
    );
    const unlistenChatEvent = listen<AgentChatEventEnvelope>(
      "agent-event",
      (event) => {
        const payload = event.payload;
        if (isAgentChatEvent(payload.event)) {
          const chatEvent = payload.event;
          setInFlightTurns((current) => {
            const turn = current[payload.sessionId];
            if (!turn) return current;
            return {
              ...current,
              [payload.sessionId]: reduceAgentChatEvent(turn, chatEvent),
            };
          });
        }
        if (payload.event.type.startsWith("compaction.")) {
          const phase = payload.event.type.slice("compaction.".length);
          if (
            phase === "started" ||
            phase === "completed" ||
            phase === "cancelled"
          ) {
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
        if (
          payload.event.type === "agent.status.updated" &&
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
        if (
          payload.event.type === "agent.status.updated" ||
          payload.event.type === "context.spliced"
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
      void unlistenDevice.then((unlisten) => unlisten());
      void unlistenBrowserError.then((unlisten) => unlisten());
      void unlistenChatEvent.then((unlisten) => unlisten());
      void unlistenInteractions.then((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => {
    const conversationId = activeConversation?.id;
    if (!conversationId) {
      setHistory(undefined);
      return;
    }

    let active = true;
    const request = (historyRequests.current[conversationId] ?? 0) + 1;
    historyRequests.current[conversationId] = request;
    setHistory({
      conversationId,
      items: [],
      hasMore: false,
      loading: true,
      loadingMore: false,
    });
    void fetchMessagePage(conversationId)
      .then((page) => {
        if (
          !active ||
          request !== historyRequests.current[conversationId]
        ) {
          return;
        }
        setHistory({
          conversationId,
          items: [...page.items].reverse(),
          hasMore: page.has_more,
          loading: false,
          loadingMore: false,
        });
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
        if (
          !active ||
          request !== historyRequests.current[conversationId]
        ) {
          return;
        }
        setHistory({
          conversationId,
          items: [],
          hasMore: false,
          loading: false,
          loadingMore: false,
          error: conciseError(error),
        });
      });

    return () => {
      active = false;
    };
  }, [activeConversation?.id]);

  useEffect(() => {
    const conversationId = activeConversation?.id;
    if (!conversationId || !auth.loggedIn) return;
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
  }, [activeConversation?.id, auth.loggedIn]);

  useLayoutEffect(() => {
    const scroll = scrollRef.current;
    if (scroll) scroll.scrollTop = scroll.scrollHeight;
  }, [activeConversation?.id, activeHistory?.loading]);

  useEffect(() => {
    const scroll = scrollRef.current;
    if (!scroll || activeHistory?.loading) return;
    scroll.scrollTo({ top: scroll.scrollHeight, behavior: "smooth" });
  }, [activeTurn?.steps, activeCompaction?.phase]);

  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    textarea.style.height = "0px";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 180)}px`;
  }, [prompt]);

  const forgetSessionState = (sessionIds: string[]): void => {
    const ids = new Set(sessionIds);
    if (ids.size === 0) return;
    for (const sessionId of ids) {
      delete historyRequests.current[sessionId];
      releaseAgentSubscription(sessionId);
    }
    setInteractions((current) => omitSessionKeys(current, ids));
    setCompactions((current) => omitSessionKeys(current, ids));
    setContextUsages((current) => omitSessionKeys(current, ids));
    setMessageDurations((current) => omitSessionKeys(current, ids));
    setPlans((current) => omitSessionKeys(current, ids));
    setInFlightTurns((current) => omitSessionKeys(current, ids));
    setHistory((current) =>
      current && ids.has(current.conversationId) ? undefined : current,
    );
    if (activeConversation && ids.has(activeConversation.id)) {
      setPrompt("");
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
        showNotice(`已从列表移除项目“${target.name}”`);
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
        showNotice(`已归档对话“${target.title}”`);
      }
      setRemovalTarget(undefined);
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setRemovalBusy(false);
    }
  };

  const addProject = async (): Promise<void> => {
    try {
      const selection = await open({
        directory: true,
        multiple: false,
        title: "选择一个项目目录",
      });
      if (!selection) return;
      const workspace = await createOrTouchWorkspace(selection);
      const existing = desktop.projects.find(
        (item) => item.id === workspace.id || item.path === selection,
      );
      if (existing) {
        updateDesktop((current) => ({
          ...current,
          activeProjectId: existing.id,
          activeConversationId: existing.conversations[0]?.id,
          projects: current.projects.map((item) => ({
            ...item,
            expanded: item.id === existing.id ? true : item.expanded,
          })),
        }));
        return;
      }
      const sessions = await listWorkspaceSessions(workspace.id);
      const project = projectFromWorkspace(
        workspace,
        desktop.projects.length,
        sessions,
      );
      updateDesktop((current) => ({
        projects: [...current.projects, project],
        activeProjectId: project.id,
        activeConversationId: undefined,
      }));
      setSidebarCollapsed(false);
    } catch (error) {
      showNotice(conciseError(error));
    }
  };

  const createConversation = async (
    project: Project,
    event?: MouseEvent<HTMLButtonElement>,
  ): Promise<void> => {
    event?.stopPropagation();
    if (!auth.loggedIn) {
      void startLogin();
      return;
    }
    const model = selectedModel ?? models[0];
    if (!model) {
      showNotice("请先同步并选择一个模型");
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
        permissionMode,
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
                conversations: [conversation, ...item.conversations],
              }
            : item,
        ),
      }));
      setPrompt("");
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
    if (!activeAgentScope) {
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
                      ? { ...conversation, modelId: effectiveModel }
                      : conversation,
                  ),
                },
          ),
        }));
        if (model?.defaultEffort) {
          await agent.setThinking(model.defaultEffort);
          setEffort(model.defaultEffort);
        }
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
    updateDesktop((current) => ({
      ...current,
      projects: current.projects.map((project) =>
        project.id !== activeProject.id
          ? project
          : {
              ...project,
              conversations: project.conversations.map((conversation) =>
                conversation.id === activeConversation.id
                  ? { ...conversation, permissionMode: mode }
                  : conversation,
              ),
            },
      ),
    }));
    if (activeAgentScope) {
      void createAgentClient(activeAgentScope)
        .setPermission(mode)
        .catch((error) => showNotice(conciseError(error)));
    }
  };

  const chooseEffort = (level: string): void => {
    setEffort(level);
    if (activeAgentScope) {
      void createAgentClient(activeAgentScope)
        .setThinking(level)
        .catch((error) => showNotice(conciseError(error)));
    }
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
      await refreshModeState(activeAgentScope);
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setModeBusy(false);
    }
  };

  const clearActivePlan = async (): Promise<void> => {
    if (!activeAgentScope || !activePlan || modeBusy) return;
    setModeBusy(true);
    try {
      await createAgentClient(activeAgentScope).clearPlan();
      await refreshModeState(activeAgentScope);
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setModeBusy(false);
    }
  };

  const startLogin = async (): Promise<void> => {
    setLoginOpen(true);
    setLoginBusy(true);
    setDeviceCode(undefined);
    try {
      const status = await invoke<AuthStatus>("login");
      setAuth(status);
      if (status.loggedIn) {
        setLoginOpen(false);
        showNotice("已登录 Kimi Code");
        await loadModels(true);
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
      releaseAllAgentSubscriptions();
      setModels([]);
      accountUsageRequest.current += 1;
      setAccountUsage(undefined);
      setAccountUsageBusy(false);
      setAccountUsageError(undefined);
      setContextUsages({});
      setMessageDurations({});
      setProfileOpen(false);
      showNotice("已退出登录");
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
      const page = await fetchMessagePage(conversationId);
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
      setHistory((current) =>
        current?.conversationId === conversationId
          ? {
              conversationId,
              items,
              hasMore: page.has_more,
              loading: false,
              loadingMore: false,
            }
          : current,
      );
      return true;
    } catch (error) {
      if (request !== historyRequests.current[conversationId]) return false;
      const message = conciseError(error);
      setHistory((current) =>
        current?.conversationId === conversationId
          ? { ...current, loading: false, loadingMore: false, error: message }
          : current,
      );
      showNotice(message);
      return false;
    }
  };

  const sendPrompt = async (override?: string): Promise<void> => {
    const text = (override ?? prompt).trim();
    if (
      !text ||
      !activeProject ||
      !activeConversation ||
      isStreaming ||
      modelBusy ||
      isHistoryLoading
    ) {
      return;
    }
    if (!auth.loggedIn) {
      void startLogin();
      return;
    }
    if (!selectedModel) {
      showNotice("请先同步并选择一个模型");
      return;
    }

    const conversationId = activeConversation.id;
    const projectId = activeProject.id;
    if (activeAgentScope?.sessionId !== conversationId) {
      showNotice("会话正在准备，请稍后再试");
      return;
    }
    const title =
      activeConversation.title === "新对话"
        ? text.replace(/\s+/g, " ").slice(0, 28)
        : activeConversation.title;

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
        activeHistory?.items.at(-1)?.id,
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
    setPrompt("");

    try {
      const launched = await createAgentClient(activeAgentScope).prompt(text);
      if (!launched) {
        setInFlightTurns((current) => {
          const turn = current[conversationId];
          if (!turn) return current;
          return {
            ...current,
            [conversationId]: {
              ...turn,
              status: "blocked",
              durationMs: Math.max(0, Date.now() - Date.parse(turn.createdAt)),
            },
          };
        });
      }
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
    void refreshHistory(conversationId, activeTurn).then((refreshed) => {
      if (!active || !refreshed) return;
      setInFlightTurns((current) => {
        if (!(conversationId in current)) return current;
        const next = { ...current };
        delete next[conversationId];
        return next;
      });
    });
    return () => {
      active = false;
    };
  }, [activeConversation?.id, activeTurn?.status]);

  const loadOlderMessages = async (): Promise<void> => {
    if (
      !activeHistory ||
      activeHistory.loading ||
      activeHistory.loadingMore ||
      !activeHistory.hasMore ||
      activeHistory.items.length === 0
    ) {
      return;
    }

    const conversationId = activeHistory.conversationId;
    const beforeId = activeHistory.items[0].id;
    const request = (historyRequests.current[conversationId] ?? 0) + 1;
    historyRequests.current[conversationId] = request;
    const scroll = scrollRef.current;
    const previousHeight = scroll?.scrollHeight ?? 0;
    setHistory((current) =>
      current?.conversationId === conversationId
        ? { ...current, loadingMore: true, error: undefined }
        : current,
    );

    try {
      const page = await fetchMessagePage(conversationId, beforeId);
      if (request !== historyRequests.current[conversationId]) return;
      const older = [...page.items].reverse();
      setHistory((current) => {
        if (current?.conversationId !== conversationId) return current;
        const loadedIds = new Set(current.items.map((message) => message.id));
        return {
          ...current,
          items: [
            ...older.filter((message) => !loadedIds.has(message.id)),
            ...current.items,
          ],
          hasMore: page.has_more,
          loadingMore: false,
        };
      });
      window.requestAnimationFrame(() => {
        if (scroll) {
          scroll.scrollTop += scroll.scrollHeight - previousHeight;
        }
      });
    } catch (error) {
      if (request !== historyRequests.current[conversationId]) return;
      const message = conciseError(error);
      setHistory((current) =>
        current?.conversationId === conversationId
          ? { ...current, loadingMore: false, error: message }
          : current,
      );
      showNotice(message);
    }
  };

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

  const handlePromptKeyDown = (
    event: KeyboardEvent<HTMLTextAreaElement>,
  ): void => {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      void sendPrompt();
    }
  };

  const copyMessage = async (message: ProtocolMessage): Promise<void> => {
    await navigator.clipboard.writeText(messageCopyText(message));
    setCopiedMessage(message.id);
    window.setTimeout(() => setCopiedMessage(undefined), 1400);
  };

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
      className={`app-shell ${
        sidebarCollapsed ? "sidebar-is-collapsed" : ""
      }`}
    >
      <WindowTitleBar
        projectName={activeProject?.name}
        conversationTitle={activeConversation?.title}
      />

      <div className="app-body">
        <aside className={sidebarCollapsed ? "sidebar collapsed" : "sidebar"}>
        <div className="brand-row">
          <div className="sidebar-heading-copy" aria-hidden={sidebarCollapsed}>
            <strong>工作区</strong>
            <span>Projects &amp; sessions</span>
          </div>
          <button
            className="icon-button quiet"
            onClick={() => {
              setSidebarCollapsed((value) => !value);
              setProfileOpen(false);
            }}
            title={sidebarCollapsed ? "展开侧栏" : "收起侧栏"}
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
              打开项目
            </span>
          </button>

          <div className="sidebar-section-heading" aria-hidden={sidebarCollapsed}>
            <span>项目</span>
          </div>

          <nav className="project-list" aria-label="项目和对话">
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
                        ? setSidebarCollapsed(false)
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
                        title="新建对话"
                        aria-label={`在 ${project.name} 中新建对话`}
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
                        title="移除项目"
                        aria-label={`移除项目 ${project.name}`}
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
                                  aria-label="对话进行中"
                                  title="对话进行中"
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
                            title="归档对话"
                            aria-label={`归档对话 ${conversation.title}`}
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
              <p>打开一个本地目录，开始和 Kimi 一起写代码。</p>
            </div>
          )}
        </div>

        <div className="account-area">
          {auth.loggedIn ? (
            <div className="profile-wrap" ref={profileRef}>
              <button
                className="account-button"
                tabIndex={sidebarCollapsed ? -1 : 0}
                aria-expanded={profileOpen}
                aria-controls="account-usage-popover"
                onClick={toggleProfile}
              >
                <span className="avatar">
                  <Sparkles size={15} />
                  <i />
                </span>
                <span className="account-copy" aria-hidden={sidebarCollapsed}>
                  <strong>Kimi Code</strong>
                  <small>已连接</small>
                </span>
                <MoreHorizontal
                  className="account-trailing-icon"
                  size={16}
                  aria-hidden={sidebarCollapsed}
                />
              </button>
              <div
                className="account-compact-actions"
                aria-hidden={!sidebarCollapsed}
                inert={!sidebarCollapsed}
              >
                <button
                  className="account-compact-kimi"
                  type="button"
                  title="Kimi Code 账号"
                  aria-label="打开 Kimi Code 账号菜单"
                  aria-expanded={profileOpen}
                  aria-controls="account-usage-popover"
                  onClick={toggleProfile}
                >
                  <Sparkles size={14} />
                </button>
              </div>
              {profileOpen && (
                <AccountUsagePopover
                  appVersion={appVersion}
                  usage={accountUsage}
                  busy={accountUsageBusy}
                  error={accountUsageError}
                  onRefresh={() => void loadAccountUsage()}
                  onSignOut={() => void signOut()}
                />
              )}
            </div>
          ) : (
            <button className="account-button login" onClick={startLogin}>
              <span className="avatar signed-out">
                <CircleUserRound size={18} />
              </span>
              <span className="account-copy" aria-hidden={sidebarCollapsed}>
                <strong>登录 Kimi</strong>
                <small>同步模型与额度</small>
              </span>
              <LogIn
                className="account-trailing-icon"
                size={16}
                aria-hidden={sidebarCollapsed}
              />
            </button>
          )}
        </div>
        </aside>

        <main className="workspace">
        {activeProject && activeConversation ? (
          <>
            <header className="chat-header">
              <div className="chat-heading">
                {sidebarCollapsed && (
                  <button
                    className="icon-button"
                    onClick={() => setSidebarCollapsed(false)}
                  >
                    <Menu size={18} />
                  </button>
                )}
                <div>
                  <h1>{activeConversation.title}</h1>
                  <div className="path-line">
                    <Folder size={12} />
                    <span>{activeProject.path}</span>
                  </div>
                </div>
              </div>
              <div className="header-actions">
                <span className="connection-pill">
                  <i className={auth.loggedIn ? "online" : ""} />
                  {auth.loggedIn ? "Core v2 已连接" : "等待登录"}
                </span>
                <button className="icon-button" title="新建对话" onClick={() => void createConversation(activeProject)}>
                  <SquarePen size={17} />
                </button>
              </div>
            </header>

            <div className="chat-scroll" ref={scrollRef}>
              {activeHistory?.loading ? (
                <div className="history-loading">
                  <span className="spinner" />
                  正在读取会话历史…
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
                <div className="message-stack">
                  {activeHistory?.hasMore && (
                    <button
                      type="button"
                      className="history-load-more"
                      disabled={activeHistory.loadingMore}
                      onClick={() => void loadOlderMessages()}
                    >
                      {activeHistory.loadingMore ? (
                        <>
                          <span className="spinner" />
                          正在加载…
                        </>
                      ) : (
                        "加载更早消息"
                      )}
                    </button>
                  )}
                  {activeHistory?.error && (
                    <div className="history-error">{activeHistory.error}</div>
                  )}
                  {historyToolPresentation.messages.map((message) => (
                    <MessageView
                      key={message.id}
                      message={message}
                      toolResults={historyToolPresentation.results}
                      durationMs={
                        messageDurations[activeConversation.id]?.[message.id]
                      }
                      copied={copiedMessage === message.id}
                      onCopy={() => void copyMessage(message)}
                    />
                  ))}
                  {activeTurn && <LiveTurnView turn={activeTurn} />}
                  {activeCompaction && (
                    <CompactionNotice event={activeCompaction} />
                  )}
                </div>
              )}
            </div>

            <div className="composer-dock">
              {activePlan && (
                <AgentModeStatus
                  plan={activePlan ?? null}
                  busy={modeBusy}
                  planLocked={isStreaming}
                  onExitPlan={() => void togglePlanMode()}
                  onClearPlan={() => void clearActivePlan()}
                />
              )}
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
              <form className="composer" onSubmit={handleSubmit}>
                <textarea
                  ref={textareaRef}
                  value={prompt}
                  onChange={(event) => setPrompt(event.target.value)}
                  onKeyDown={handlePromptKeyDown}
                  placeholder={
                    activePlan
                      ? "计划模式：描述需要分析和规划的任务…"
                      : auth.loggedIn
                      ? "告诉 Kimi 你想完成什么…"
                      : "登录后开始与 Kimi Code 对话…"
                  }
                  rows={1}
                  disabled={isStreaming || modelBusy || hasBlockingInteraction}
                />
                <div className="composer-toolbar">
                  <div className="composer-options">
                    <ToolbarSelect
                      className="model-select"
                      ariaLabel="选择模型"
                      icon={<Bot size={15} />}
                      value={selectedModel?.id ?? ""}
                      label={
                        modelsBusy
                          ? "同步模型中…"
                          : selectedModel?.displayName ??
                            (auth.loggedIn ? "暂无模型" : "登录后选择模型")
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
                        description: `${formatContext(model.contextLength)} 上下文${
                          model.supportsReasoning ? " · 支持深度思考" : ""
                        }`,
                      }))}
                      onChange={chooseModel}
                    />
                    {auth.loggedIn && (
                      <button
                        className="toolbar-icon"
                        type="button"
                        title="刷新模型列表"
                        onClick={() => void loadModels(true)}
                        disabled={modelsBusy}
                      >
                        <RefreshCw size={14} />
                      </button>
                    )}
                    {selectedModel?.supportsReasoning && (
                      <ToolbarSelect
                        className="effort-select"
                        ariaLabel="选择思考强度"
                        icon={<BrainCircuit size={15} />}
                        value={effort}
                        label={`思考 · ${effort}`}
                        options={(selectedModel.supportEfforts.length
                          ? selectedModel.supportEfforts
                          : ["low", "medium", "high"]
                        ).map((value) => ({
                          value,
                          label: `思考 · ${value}`,
                          description:
                            value === "low"
                              ? "快速响应，适合简单任务"
                              : value === "high"
                                ? "更深入分析复杂问题"
                                : "速度与推理深度平衡",
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
                      ariaLabel="选择权限模式"
                      icon={<ShieldCheck size={15} />}
                      value={permissionMode}
                      label={
                        permissionMode === "yolo"
                          ? "完全访问"
                          : permissionMode === "auto"
                            ? "自动选择"
                            : "请求审批"
                      }
                      disabled={isStreaming || modelBusy}
                      options={[
                        {
                          value: "manual",
                          label: "请求审批",
                          description: "执行命令前由你确认",
                        },
                        {
                          value: "auto",
                          label: "自动选择",
                          description: "由权限策略判断是否允许",
                        },
                        {
                          value: "yolo",
                          label: "完全访问",
                          description: "跳过审批并直接执行命令",
                          danger: true,
                        },
                      ]}
                      onChange={(value) =>
                        choosePermissionMode(value as PermissionMode)
                      }
                    />
                    <button
                      className={`mode-toolbar-button plan-mode ${
                        activePlan ? "active" : ""
                      }`}
                      type="button"
                      disabled={!activeAgentScope || modeBusy || isStreaming}
                      onClick={() => void togglePlanMode()}
                      title={activePlan ? "退出计划模式" : "进入计划模式"}
                      aria-pressed={Boolean(activePlan)}
                    >
                      <ClipboardList size={14} />
                      <span>{activePlan ? "计划中" : "计划"}</span>
                    </button>
                    {selectedModel && (
                      <ContextUsageIndicator
                        usage={activeContextUsage}
                        maxContextTokens={selectedModel.contextLength}
                      />
                    )}
                  </div>
                  <div className="send-zone">
                    <span>Enter 发送</span>
                    <button
                      className="send-button"
                      type={isStreaming ? "button" : "submit"}
                      onClick={
                        isStreaming ? () => void cancelActiveTurn() : undefined
                      }
                      disabled={
                        isStreaming
                          ? !activeAgentScope
                          : hasBlockingInteraction ||
                            !prompt.trim() ||
                            isHistoryLoading ||
                            modelBusy ||
                            !activeAgentScope
                      }
                      title="发送"
                    >
                      {isStreaming ? <X size={17} /> : <ArrowUp size={18} />}
                    </button>
                  </div>
                </div>
              </form>
              <p className="composer-caption">
                Kimi 可能会犯错，请检查生成的代码和重要信息。
              </p>
            </div>
          </>
        ) : (
          <ProjectLanding
            collapsed={sidebarCollapsed}
            onExpand={() => setSidebarCollapsed(false)}
            onAddProject={() => void addProject()}
          />
        )}
        </main>
      </div>

      {loginOpen && (
        <LoginDialog
          busy={loginBusy}
          code={deviceCode}
          onClose={() => !loginBusy && setLoginOpen(false)}
          onStart={() => void startLogin()}
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

      {notice && (
        <div className="toast" role="status">
          <span>{notice}</span>
          <button aria-label="关闭提示" onClick={() => setNotice(undefined)}>
            <X size={14} />
          </button>
        </div>
      )}
    </div>
  );
}

function AccountUsagePopover({
  appVersion,
  usage,
  busy,
  error,
  onRefresh,
  onSignOut,
}: {
  appVersion?: string;
  usage?: AccountUsage;
  busy: boolean;
  error?: string;
  onRefresh: () => void;
  onSignOut: () => void;
}) {
  const rows = usage
    ? [...(usage.summary ? [usage.summary] : []), ...usage.limits]
    : [];

  return (
    <div
      id="account-usage-popover"
      className="profile-popover"
      role="dialog"
      aria-label="Kimi Code 账号用量"
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
            <small>OAuth 账号</small>
          </span>
        </div>
        <button
          className="profile-refresh"
          type="button"
          title="刷新账号用量"
          aria-label="刷新账号用量"
          disabled={busy}
          onClick={onRefresh}
        >
          <RefreshCw className={busy ? "spinning" : ""} size={13} />
        </button>
      </div>

      <div className="account-usage-content" aria-live="polite">
        <div className="account-usage-heading">
          <span>套餐用量</span>
          {busy && usage && <small>正在更新</small>}
        </div>

        {busy && !usage ? (
          <div className="account-usage-skeleton" aria-label="正在加载账号用量">
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
            {error ? "额度暂时无法加载" : "当前账号未返回套餐额度"}
          </div>
        )}

        {error && (
          <div className="account-usage-error">
            <span>{error}</span>
            <button type="button" disabled={busy} onClick={onRefresh}>
              重试
            </button>
          </div>
        )}

        {usage?.extraUsage && (
          <BoosterWalletSummary wallet={usage.extraUsage} />
        )}
      </div>

      <div className="profile-popover-footer">
        <button className="profile-signout" type="button" onClick={onSignOut}>
          <LogOut size={14} />
          退出登录
        </button>
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
      <div className="managed-usage-meta">
        <span>
          {formatUsageValue(used)} / {formatUsageValue(limit)}
        </span>
        {row.resetHint && <span>{formatResetHint(row.resetHint)}</span>}
      </div>
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
        <span>额外用量</span>
        <small>Booster</small>
      </div>
      <div className="booster-balance">
        <span>可用余额</span>
        <strong>{formatCurrency(wallet.balanceCents, wallet.currency)}</strong>
      </div>
      <div className="booster-details">
        <span>
          本月已用 {formatCurrency(wallet.monthlyUsedCents, wallet.currency)}
        </span>
        <span>
          {hasMonthlyLimit
            ? `上限 ${formatCurrency(wallet.monthlyChargeLimitCents, wallet.currency)}`
            : "月度上限：不限"}
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
  if (normalized === "weekly limit") return "每周额度";
  return label
    .replace(/^(\d+)h limit$/i, "$1 小时额度")
    .replace(/^(\d+)d limit$/i, "$1 天额度")
    .replace(/^(\d+)m limit$/i, "$1 分钟额度");
}

function formatResetHint(hint: string): string {
  if (hint === "reset") return "已重置";
  if (hint.startsWith("resets in ")) return `${hint.slice(10)} 后重置`;
  if (hint.startsWith("resets at ")) return `${hint.slice(10)} 重置`;
  return hint;
}

function formatUsageValue(value: number): string {
  return new Intl.NumberFormat("zh-CN", {
    maximumFractionDigits: Number.isInteger(value) ? 0 : 1,
  }).format(value);
}

function formatCurrency(cents: number, currency: string): string {
  try {
    return new Intl.NumberFormat("zh-CN", {
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

function WindowTitleBar({
  projectName,
  conversationTitle,
}: {
  projectName?: string;
  conversationTitle?: string;
}) {
  const [maximized, setMaximized] = useState(false);
  const appWindow = useMemo(
    () => (isTauri() ? getCurrentWindow() : undefined),
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

  const contextTitle =
    conversationTitle ?? projectName ?? "准备开始新的编码任务";

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
          <span data-tauri-drag-region>Agent Desktop</span>
        </div>
      </div>

      <div className="window-titlebar-context" data-tauri-drag-region>
        <span className="window-context-project" data-tauri-drag-region>
          <i data-tauri-drag-region />
          <span data-tauri-drag-region>{projectName ?? "Local workspace"}</span>
        </span>
        <span className="window-context-divider" data-tauri-drag-region />
        <strong data-tauri-drag-region title={contextTitle}>
          {contextTitle}
        </strong>
      </div>

      <div className="window-controls">
        <button
          className="window-control"
          type="button"
          title="最小化"
          aria-label="最小化窗口"
          onClick={() => runWindowAction("minimize")}
        >
          <Minus size={15} strokeWidth={1.7} />
        </button>
        <button
          className="window-control"
          type="button"
          title={maximized ? "还原" : "最大化"}
          aria-label={maximized ? "还原窗口" : "最大化窗口"}
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
          title="关闭"
          aria-label="关闭窗口"
          onClick={() => runWindowAction("close")}
        >
          <X size={15} strokeWidth={1.7} />
        </button>
      </div>
    </header>
  );
}

function ContextUsageIndicator({
  usage,
  maxContextTokens,
}: {
  usage?: ContextUsage;
  maxContextTokens: number;
}) {
  const contextTokens = Math.max(0, usage?.contextTokens ?? 0);
  const effectiveMax =
    maxContextTokens > 0 ? maxContextTokens : (usage?.maxContextTokens ?? 0);
  const ratio = effectiveMax > 0 ? contextTokens / effectiveMax : 0;
  const progress = Math.min(1, Math.max(0, ratio));
  const percent = effectiveMax > 0 ? Math.round(ratio * 100) : undefined;
  const level = ratio >= 0.85 ? "critical" : ratio >= 0.7 ? "warning" : "";

  return (
    <div
      className={`context-usage ${level}`}
      tabIndex={0}
      aria-label={
        percent === undefined
          ? "上下文窗口上限未知"
          : `上下文窗口已用 ${percent}%`
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
        <strong>上下文窗口</strong>
        <span className="context-usage-summary">
          {percent === undefined ? "使用量未知" : `${percent}% 已用`}
        </span>
        <span>
          已用 {formatTokenCount(contextTokens)} tokens，共{" "}
          {effectiveMax > 0 ? formatTokenCount(effectiveMax) : "未知"}
        </span>
      </div>
    </div>
  );
}

function AgentModeStatus({
  plan,
  busy,
  planLocked,
  onExitPlan,
  onClearPlan,
}: {
  plan: PlanData | null;
  busy: boolean;
  planLocked: boolean;
  onExitPlan: () => void;
  onClearPlan: () => void;
}) {
  const planPreview =
    plan?.content
      .split(/\r?\n/)
      .map((line) => line.replace(/^#+\s*/, "").trim())
      .find(Boolean) ?? "先分析和制定方案，不直接修改代码";

  return (
    <section className="mode-status-stack" aria-label="Agent 工作模式">
      {plan && (
        <article className="mode-status-card plan">
          <span className="mode-status-icon">
            <ClipboardList size={15} />
          </span>
          <span className="mode-status-copy">
            <span className="mode-status-heading">
              <strong>计划模式</strong>
              <small>只规划，不执行</small>
            </span>
            <span className="mode-status-detail">{planPreview}</span>
          </span>
          <span className="mode-status-actions">
            <button
              type="button"
              onClick={onClearPlan}
              disabled={busy || planLocked}
              title="清空计划内容"
            >
              <Trash2 size={13} />
            </button>
            <button
              type="button"
              onClick={onExitPlan}
              disabled={busy || planLocked}
              title="退出计划模式"
            >
              <X size={13} />
              <span>退出</span>
            </button>
          </span>
        </article>
      )}
    </section>
  );
}

function CompactionNotice({ event }: { event: CompactionEvent }) {
  const completed = event.phase === "completed";
  const cancelled = event.phase === "cancelled";
  const detail = completed
    ? event.tokensBefore !== undefined && event.tokensAfter !== undefined
      ? `${formatContext(Math.round(event.tokensBefore))} → ${formatContext(
          Math.round(event.tokensAfter),
        )} tokens${
          event.compactedCount !== undefined
            ? ` · 整理 ${Math.round(event.compactedCount)} 条上下文`
            : ""
        }`
      : "较早的对话已整理为上下文摘要"
    : cancelled
      ? "本次上下文整理未完成，对话内容保持不变"
      : `${
          event.trigger === "auto" ? "自动触发" : "手动触发"
        } · 正在将较早的对话整理为摘要`;

  return (
    <div className={`compaction-notice ${event.phase}`} role="status">
      <span className="compaction-glyph">
        {completed ? (
          <Check size={14} />
        ) : cancelled ? (
          <X size={14} />
        ) : (
          <Minimize2 size={14} />
        )}
      </span>
      <span>
        <strong>
          {completed
            ? "上下文压缩完成"
            : cancelled
              ? "上下文压缩已取消"
              : "正在压缩上下文"}
        </strong>
        <small>{detail}</small>
      </span>
      {event.phase === "started" && <i />}
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
          <small>Kimi 需要你补充信息</small>
          <strong>回答后将继续制定计划</strong>
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
                <span>{question.otherLabel || "其他"}</span>
                <input
                  value={otherAnswers[questionIndex] ?? ""}
                  disabled={busy}
                  placeholder={question.otherDescription || "输入其他答案"}
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
          跳过
        </button>
        <button
          type="button"
          className="interaction-primary"
          disabled={busy || !canSubmit}
          onClick={submit}
        >
          {busy ? <span className="spinner light" /> : <Check size={14} />}
          提交回答
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

  return (
    <section className="interaction-card plan-review-card" aria-live="polite">
      <div className="interaction-card-heading">
        <span className="interaction-card-icon">
          <ClipboardList size={18} />
        </span>
        <div>
          <small>计划已完成</small>
          <strong>审核计划并选择下一步</strong>
        </div>
        <span className="approval-tool">{payload.toolName}</span>
      </div>
      <div className="plan-review-content">
        <ReactMarkdown remarkPlugins={[remarkGfm]}>{display.plan}</ReactMarkdown>
      </div>
      {display.path && <code className="plan-review-path">{display.path}</code>}
      {options.length > 0 && (
        <div className="plan-review-options">
          <span>选择实施方案</span>
          {options.map((option) => (
            <button
              type="button"
              key={option.label}
              disabled={busy}
              onClick={() =>
                onRespond({
                  decision: "approved",
                  selectedLabel: option.label,
                })
              }
            >
              <strong>{option.label}</strong>
              {option.description && <small>{option.description}</small>}
            </button>
          ))}
        </div>
      )}
      <label className="plan-review-feedback">
        <span>需要调整？写下修改意见</span>
        <textarea
          rows={2}
          value={feedback}
          disabled={busy}
          placeholder="告诉 Kimi 需要修改计划的哪些部分"
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
          拒绝
        </button>
        <button
          type="button"
          className="interaction-secondary"
          disabled={busy || !feedback.trim()}
          onClick={() =>
            onRespond({
              decision: "rejected",
              selectedLabel: "Revise",
              feedback: feedback.trim(),
            })
          }
        >
          退回修改
        </button>
        {options.length === 0 && (
          <button
            type="button"
            className="interaction-primary"
            disabled={busy}
            onClick={() =>
              onRespond({ decision: "approved", selectedLabel: "Approve" })
            }
          >
            {busy ? <span className="spinner light" /> : <Check size={14} />}
            批准并继续
          </button>
        )}
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
            <span>需要你的批准</span>
            <strong>{payload.action || `${payload.toolName} 请求执行操作`}</strong>
          </div>
          <span className="approval-tool">{payload.toolName}</span>
        </div>
        {command ? (
          <div className="approval-command">
            <div>
              <TerminalSquare size={13} />
              <span>{cwd || "当前项目目录"}</span>
            </div>
            <code>{command}</code>
          </div>
        ) : (
          <div className="approval-detail">{String(detail || "该操作需要确认")}</div>
        )}
        <div className="approval-footer">
          <p>请确认命令及工作目录可信后再允许执行。</p>
          <div className="approval-actions">
            <button type="button" className="approval-reject" onClick={onReject} disabled={busy}>
              拒绝
            </button>
            <button type="button" className="approval-session" onClick={onApproveSession} disabled={busy}>
              本会话允许
            </button>
            <button type="button" className="approval-once" onClick={onApprove} disabled={busy}>
              {busy ? <span className="spinner light" /> : <Check size={14} />}
              允许一次
            </button>
          </div>
        </div>
      </div>
    </section>
  );
}

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
        准备好一起构建
        <br />
        <span>{project.name}</span> 了吗？
      </h2>
      <p className="welcome-copy">
        我会结合当前项目上下文理解你的目标。你可以让我阅读代码、解释结构，或从一个具体任务开始。
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

function LiveTurnView({ turn }: { turn: InFlightTurn }) {
  const hasBlocks = turn.steps.some((step) => step.blocks.length > 0);

  return (
    <>
      <article className="message user-message live-user-message">
        <div className="message-meta">
          <span>你</span>
          <time>{formatTime(turn.createdAt)}</time>
        </div>
        <div className="user-bubble">{turn.prompt}</div>
      </article>
      <article className={`message assistant-message live-turn ${turn.status}`}>
        <div className="assistant-rail">
          <span className="assistant-avatar">
            <Sparkles size={15} />
          </span>
          <i />
        </div>
        <div className="assistant-body">
          <div className="message-meta">
            <span>Kimi</span>
            <time>{formatTime(turn.createdAt)}</time>
            <span className={`live-turn-status ${turn.status}`}>
              {liveTurnStatusLabel(turn.status)}
            </span>
          </div>
          {turn.steps.map((step) => (
            <section
              className={`live-step ${step.status}`}
              key={step.stepId ?? step.step}
            >
              {step.blocks.map((block, index) => {
                if (block.kind === "text") {
                  return (
                    <div className="markdown-body live-text" key={index}>
                      <MarkdownMessage content={block.content} />
                    </div>
                  );
                }
                if (block.kind === "thinking") {
                  return (
                    <LiveThinkingBlock content={block.content} key={index} />
                  );
                }
                if (block.kind === "content") {
                  return (
                    <LiveAssistantContent
                      content={block.content}
                      key={index}
                    />
                  );
                }
                return <LiveToolBlock tool={block} key={block.toolCallId} />;
              })}
              {step.interruption && (
                <div className="live-step-interruption">{step.interruption}</div>
              )}
            </section>
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
    </>
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
          <span>正在思考</span>
          <span className="assistant-thinking-dots" aria-hidden="true">
            <i />
            <i />
            <i />
          </span>
        </>
      ) : (
        <span>用时 {formatElapsedDuration(durationMs ?? 0)}</span>
      )}
    </div>
  );
}

function liveTurnStatusLabel(status: LiveTurnStatus): string {
  switch (status) {
    case "queued":
      return "等待中";
    case "running":
      return "进行中";
    case "completed":
      return "已完成";
    case "cancelled":
      return "已取消";
    case "failed":
      return "失败";
    case "blocked":
      return "已阻止";
  }
}

function LiveThinkingBlock({ content }: { content: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="thinking-block live-thinking">
      <button onClick={() => setOpen((value) => !value)}>
        <BrainCircuit size={14} />
        <span>思考过程</span>
        {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
      </button>
      {open && <p>{content}</p>}
    </div>
  );
}

function LiveAssistantContent({ content }: { content: AgentContentPart }) {
  switch (content.type) {
    case "text":
      return (
        <div className="markdown-body live-text">
          <MarkdownMessage content={content.text} />
        </div>
      );
    case "think":
      return <LiveThinkingBlock content={content.think} />;
    case "image_url":
      return (
        <img
          className="history-media"
          src={content.imageUrl.url}
          alt="对话图片"
        />
      );
    case "audio_url":
      return (
        <div className="markdown-body">
          <MarkdownMessage content={`[audio:${content.audioUrl.url}]`} />
        </div>
      );
    case "video_url":
      return (
        <div className="markdown-body">
          <MarkdownMessage content={`[video:${content.videoUrl.url}]`} />
        </div>
      );
  }
}

function LiveToolBlock({
  tool,
}: {
  tool: Extract<LiveBlock, { kind: "tool" }>;
}) {
  const [open, setOpen] = useState(
    tool.status === "streaming" || tool.status === "running",
  );
  useEffect(() => {
    setOpen(tool.status === "streaming" || tool.status === "running");
  }, [tool.status]);
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
  return (
    <details
      className={`live-tool-card ${tool.status}`}
      open={open}
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary>
        <ToolStatusIcon status={tool.status} />
        <Wrench size={13} />
        <span>{tool.name ?? "准备工具调用"}</span>
        <small>{liveToolStatusLabel(tool.status)}</small>
      </summary>
      <div className="live-tool-detail">
        {tool.description && <p>{tool.description}</p>}
        {input !== undefined && (
          <section className="tool-detail-section">
            <span>参数</span>
            <pre>{structuredValue(input)}</pre>
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
            <span>结果</span>
            <pre className={tool.isError ? "error" : ""}>
              {structuredValue(tool.output)}
            </pre>
          </section>
        )}
      </div>
    </details>
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
      return "准备中";
    case "running":
      return "执行中";
    case "completed":
      return "已完成";
    case "error":
      return "失败";
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
    return <span className="tool-status-icon spinning" aria-label="执行中" />;
  }
  if (status === "completed") {
    return (
      <span className="tool-status-icon completed" aria-label="已完成">
        <Check size={11} />
      </span>
    );
  }
  if (status === "error") {
    return (
      <span className="tool-status-icon error" aria-label="执行失败">
        <X size={11} />
      </span>
    );
  }
  return (
    <span className="tool-status-icon incomplete" aria-label="未完成">
      <MoreHorizontal size={11} />
    </span>
  );
}

function MessageView({
  message,
  toolResults,
  durationMs,
  copied,
  onCopy,
}: {
  message: RenderMessage;
  toolResults: Map<string, ToolResultContent>;
  durationMs?: number;
  copied: boolean;
  onCopy: () => void;
}) {
  const [thinkingOpen, setThinkingOpen] = useState(false);
  const text = messageText(message);
  const thinking = messageThinking(message);
  const structured = message.content.filter(
    (part) => part.type !== "text" && part.type !== "thinking",
  );
  const origin = message.metadata?.origin;
  const originKind =
    origin && typeof origin === "object" && "kind" in origin
      ? String(origin.kind)
      : undefined;

  if (originKind === "compaction_summary") {
    return (
      <details className="history-summary">
        <summary>
          <BrainCircuit size={14} />
          上下文已压缩
        </summary>
        <div className="markdown-body">
          <MarkdownMessage content={text} />
        </div>
      </details>
    );
  }

  if (message.role === "user") {
    return (
      <article className="message user-message">
        <div className="message-meta">
          <span>你</span>
          <time>{formatTime(message.created_at)}</time>
        </div>
        <div className="user-bubble">
          {text}
          <StructuredMessageContent
            parts={structured}
            toolResults={toolResults}
          />
        </div>
      </article>
    );
  }

  const author =
    message.role === "tool"
      ? "工具"
      : message.role === "system"
        ? "系统"
        : "Kimi";
  return (
    <article className={`message assistant-message ${message.status ?? ""}`}>
      <div className="assistant-rail">
        <span className="assistant-avatar">
          {message.role === "assistant" ? (
            <Sparkles size={15} />
          ) : (
            <TerminalSquare size={15} />
          )}
        </span>
        <i />
      </div>
      <div className="assistant-body">
        <div className="message-meta">
          <span>{author}</span>
          <time>{formatTime(message.created_at)}</time>
        </div>
        {thinking && (
          <div className="thinking-block">
            <button onClick={() => setThinkingOpen((value) => !value)}>
              <BrainCircuit size={14} />
              <span>思考过程</span>
              {thinkingOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
            </button>
            {thinkingOpen && <p>{thinking}</p>}
          </div>
        )}
        <div className="markdown-body">
          {text ? (
            <MarkdownMessage content={text} />
          ) : structured.length > 0 ? null : (
            <div className="typing">
              <i />
              <i />
              <i />
            </div>
          )}
          <StructuredMessageContent
            parts={structured}
            toolResults={toolResults}
          />
        </div>
        {message.status !== "streaming" &&
          messageCopyText(message).length > 0 && (
            <div className="message-actions">
              <button onClick={onCopy}>
                {copied ? <Check size={14} /> : <Copy size={14} />}
                {copied ? "已复制" : "复制"}
              </button>
            </div>
          )}
        {message.role === "assistant" && durationMs !== undefined && (
          <AssistantResponseStatus running={false} durationMs={durationMs} />
        )}
      </div>
    </article>
  );
}

function messageText(message: ProtocolMessage): string {
  return message.content
    .filter(
      (part): part is Extract<MessageContent, { type: "text" }> =>
        part.type === "text",
    )
    .map((part) => part.text)
    .join("");
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

function messageCopyText(message: ProtocolMessage): string {
  return message.content
    .map((part) => {
      switch (part.type) {
        case "text":
          return part.text;
        case "thinking":
          return part.thinking;
        case "tool_use":
          return `${part.tool_name}\n${structuredValue(part.input)}`;
        case "tool_result":
          return structuredValue(part.output);
        case "image":
        case "video":
          return part.source.kind === "url" ? part.source.url : "";
        case "file":
          return part.name;
      }
    })
    .filter(Boolean)
    .join("\n\n");
}

function mediaSourceUrl(
  source: Extract<MessageContent, { type: "image" | "video" }>["source"],
): string | undefined {
  if (source.kind === "url") return source.url;
  if (source.kind === "base64") {
    return `data:${source.media_type};base64,${source.data}`;
  }
  return undefined;
}

function StructuredMessageContent({
  parts,
  toolResults,
}: {
  parts: MessageContent[];
  toolResults: Map<string, ToolResultContent>;
}) {
  if (parts.length === 0) return null;
  return (
    <div className="structured-content">
      {parts.map((part, index) => {
        switch (part.type) {
          case "tool_use": {
            const result = toolResults.get(part.tool_call_id);
            const status = result
              ? result.is_error
                ? "error"
                : "completed"
              : "incomplete";
            return (
              <details
                className={`history-tool-card ${status}`}
                key={`${part.tool_call_id}-${index}`}
              >
                <summary>
                  <ToolStatusIcon status={status} />
                  <Wrench size={13} />
                  <span>{part.tool_name}</span>
                  <small>
                    {result
                      ? result.is_error
                        ? "失败"
                        : "已完成"
                      : "未完成"}
                  </small>
                </summary>
                <div className="history-tool-detail">
                  <section className="tool-detail-section">
                    <span>参数</span>
                    <pre>{structuredValue(part.input)}</pre>
                  </section>
                  {result && (
                    <section className="tool-detail-section">
                      <span>结果</span>
                      <pre className={result.is_error ? "error" : ""}>
                        {structuredValue(result.output)}
                      </pre>
                    </section>
                  )}
                </div>
              </details>
            );
          }
          case "tool_result":
            return null;
          case "image": {
            const url = mediaSourceUrl(part.source);
            return url ? (
              <img className="history-media" src={url} alt="会话图片" key={index} />
            ) : (
              <div className="history-file" key={index}>
                图片文件：{part.source.kind === "file" ? part.source.file_id : ""}
              </div>
            );
          }
          case "video": {
            const url = mediaSourceUrl(part.source);
            return url ? (
              <video className="history-media" src={url} controls key={index} />
            ) : (
              <div className="history-file" key={index}>
                视频文件：{part.source.kind === "file" ? part.source.file_id : ""}
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

function MarkdownMessage({ content }: { content: string }) {
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
        a({ children, ...props }) {
          return (
            <a {...props} target="_blank" rel="noreferrer">
              {children}
            </a>
          );
        },
      }}
    >
      {content}
    </ReactMarkdown>
  );
}

function ProjectLanding({
  collapsed,
  onExpand,
  onAddProject,
}: {
  collapsed: boolean;
  onExpand: () => void;
  onAddProject: () => void;
}) {
  return (
    <div className="project-landing">
      {collapsed && (
        <button className="landing-menu icon-button" onClick={onExpand}>
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
      <h1>从一个项目开始</h1>
      <p>
        选择本地代码目录。每个项目都有独立的对话空间，
        <br />
        你的上下文和灵感会一直留在这里。
      </p>
      <button className="landing-primary" onClick={onAddProject}>
        <Folder size={17} />
        打开本地项目
      </button>
      <div className="landing-shortcut">
        <span>提示</span>
        你也可以把项目文件夹拖到窗口中
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
          aria-label="关闭确认窗口"
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
          {isProject ? "移除这个项目？" : "归档这个对话？"}
        </h2>
        <p className="dialog-copy">
          {isProject
            ? `“${target.name}”只会从项目列表中移除，本地目录和历史对话都不会被删除。重新打开该目录即可恢复。`
            : `“${target.title}”将移入归档并从当前列表隐藏，对话内容不会从磁盘永久删除。`}
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
            取消
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
                正在处理…
              </>
            ) : isProject ? (
              <>
                <FolderMinus size={15} />
                移除项目
              </>
            ) : (
              <>
                <Archive size={15} />
                归档对话
              </>
            )}
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
          aria-label="关闭登录窗口"
          onClick={onClose}
          disabled={busy}
        >
          <X size={17} />
        </button>
        <div className="login-logo">
          <Sparkles size={24} />
        </div>
        <p className="eyebrow">KIMI CODE ACCOUNT</p>
        <h2>连接你的 Kimi 账号</h2>
        <p className="dialog-copy">
          登录后会安全地同步可用模型。授权信息由 agent-core-v2 保存在本机。
        </p>
        {code ? (
          <>
            <button className="device-code" onClick={() => void copyCode()}>
              <span>设备验证码</span>
              <strong>{code.userCode}</strong>
              <small>{copied ? "已复制" : "点击复制"}</small>
            </button>
            <button
              className="dialog-primary"
              onClick={() => void openUrl(code.verificationUriComplete || code.verificationUri)}
            >
              在浏览器中授权
              <ExternalLink size={16} />
            </button>
            <div className="waiting-line">
              <span className="spinner" />
              等待浏览器确认…
            </div>
          </>
        ) : (
          <>
            <div className="login-features">
              <span><Check size={14} /> OAuth 安全登录</span>
              <span><Check size={14} /> 自动同步模型</span>
              <span><Check size={14} /> 凭证仅保存在本机</span>
            </div>
            <button className="dialog-primary" onClick={onStart} disabled={busy}>
              {busy ? (
                <>
                  <span className="spinner light" />
                  正在创建授权…
                </>
              ) : (
                <>
                  继续登录
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
