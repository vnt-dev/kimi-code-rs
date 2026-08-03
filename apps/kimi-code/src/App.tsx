import {
  type CSSProperties,
  type FormEvent,
  type KeyboardEvent,
  type MouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ClipboardEvent,
  type ChangeEvent,
  type UIEvent as ReactUIEvent,
  type WheelEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  Archive,
  ArrowUp,
  Bot,
  BrainCircuit,
  Check,
  ChevronRight,
  CircleUserRound,
  ClipboardList,
  Copy,
  FileCode2,
  Folder,
  FolderGit2,
  FolderMinus,
  LogIn,
  Menu,
  MessageSquareText,
  Minimize2,
  MoreHorizontal,
  Package,
  Paperclip,
  PanelLeftClose,
  PanelLeftOpen,
  Pause,
  Play,
  Plus,
  ShieldCheck,
  Sparkles,
  SquarePen,
  Target,
  X,
} from "lucide-react";
import {
  archiveSession,
  createAgentClient,
  createOrTouchWorkspace,
  forkSession,
  getSharedGoalMode,
  getSkillContent,
  listSkills,
  listWorkspaceSessions,
  prepareSession,
  removeWorkspace,
  setDefaultModel,
  setSharedGoalMode,
  subscribeAgentEvents,
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
import {
  isSameLiveUserMessage,
  projectLiveUserMessage,
} from "./liveUserMessage";
import {
  isUpwardChatScrollKey,
  resolveChatFollowState,
} from "./chatScroll";
import { buildSkillPromptText, parseSkillPromptDisplay } from "./prompt/skills";
import {
  MAX_PROMPT_ATTACHMENTS,
  buildAgentPromptInput,
  preparePromptAttachment,
  promptAttachmentKind,
} from "./prompt/attachments";
import { conciseError } from "./utils/errors";
import {
  formatBytes,
  formatContext,
} from "./utils/format";
import {
  MAIN_AGENT_ID,
  inFlightTurnFromUserMessage,
  isTurnRunning,
  liveTurnStatusFromSubmit,
  newInFlightTurn,
  readPromptSubmittedEvent,
  reduceAgentChatEvent,
  reduceQueuedAgentChatEvents,
  reduceQueuedSubagentChatEvents,
  type GoalModeChangedEvent,
  type InFlightTurn,
  type PromptAttachment,
  type QueuedAgentChatEvent,
  type QueuedPrompt,
  type RemoteQueuedPrompt,
  type SubagentLiveTurns,
} from "./chat/liveTurns";
import {
  displayMessageText,
  messageText,
} from "./chat/messages";
import {
  completedTurnMessageId,
  finalResponseMessage,
  groupHistoryMessages,
  historyBeforeInFlightTurn,
  isDirectUserMessage,
  isVisibleHistoryMessage,
  mergeHistoryToolResults,
  messageOriginKind,
  type RenderMessage,
} from "./chat/history";
import {
  isAgentChatEvent,
  isTaskLifecycleEventType,
  readAgentTaskInfo,
  readTodoItems,
} from "./chat/eventParsing";
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
import { AccountUsagePopover } from "./components/AccountUsagePopover";
import { RemixSparklingLineIcon } from "./components/RemixSparklingLineIcon";
import { ProjectLanding } from "./components/ProjectLanding";
import {
  BackgroundTaskProgress,
  ChatHeaderTitle,
  CompactionNotice,
  ContextUsageIndicator,
  TodoProgress,
  ToolbarSelect,
  WindowTitleBar,
} from "./components/ChatHeader";
import {
  DirectoryPickerDialog,
  GoalEditDialog,
  LoginDialog,
  RemovalDialog,
  UndoMessageDialog,
  WebCredentialDialog,
  type RemovalTarget,
} from "./components/AppDialogs";
import {
  ConversationOutline,
  compactOutlineText,
  conversationOutlinePreview,
  outlineTickWidth,
  type ConversationOutlineItem,
} from "./components/chat/ConversationOutline";
import {
  ApprovalCard,
  PlanReviewCard,
  QuestionCard,
  isPlanReviewInteraction,
} from "./components/InteractionCards";
import {
  HistoryTurnView,
  LiveTurnView,
  QueuedPromptList,
  RemoteQueuedPromptList,
  Welcome,
} from "./components/chat/ConversationViews";
import {
  CompactionSummarySidebar,
  SideChatSidebar,
  SkillDetailSidebar,
  type CompactionSummaryDetail,
  type SideChatState,
  type SkillDetailTarget,
} from "./components/sidebars/ChatSidebars";
import {
  TRANSPORT_AUTH_REQUIRED,
  TRANSPORT_REPLAY_RESET,
  getAppVersion,
  invoke,
  isDesktop,
  listen,
  pickNativeDirectory,
  setWebCredential,
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
  saveLanguage,
  setLanguage,
  t,
  type Language,
} from "./i18n";
import {
  isSubagentEvent,
  mergeSessionSubagentEvent,
  type SessionSubagentRuns,
} from "./subagentEvents";
import type {
  AccountUsage,
  AgentChatEventEnvelope,
  AgentInteraction,
  AgentInteractionsEvent,
  AgentUsageStatus,
  AuthStatus,
  BackgroundTaskView,
  CompactionEvent,
  ContextUsage,
  DesktopState,
  DeviceCode,
  GoalSnapshot,
  MessagePage,
  Model,
  PermissionMode,
  PlanData,
  Project,
  ProtocolMessage,
  SkillContent,
  SkillDescriptor,
  TodoItem,
  TurnFileChange,
} from "./types";

const MAX_PROMPT_SKILLS = 8;
const SLASH_COMMAND_COUNT = 3;
const LIVE_TURN_HANDOFF_MS = 200;
const BACKGROUND_TASK_LIST_LIMIT = 50;
const BACKGROUND_TASK_OUTPUT_TAIL = 16_384;
const BACKGROUND_TASK_DETAIL_TAIL = 65_536;
interface ConversationHistory {
  conversationId: string;
  items: ProtocolMessage[];
  loading: boolean;
  error?: string;
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

function fetchConversationHistory(
  conversationId: string,
): Promise<MessagePage> {
  return invoke<MessagePage>("list_conversation_messages", {
    sessionId: conversationId,
  });
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

function nestedVerticalScroller(
  target: EventTarget | null,
  root: HTMLElement,
): HTMLElement | undefined {
  let element = target instanceof HTMLElement ? target : undefined;
  while (element && element !== root) {
    const overflowY = window.getComputedStyle(element).overflowY;
    if (
      element.scrollHeight > element.clientHeight + 1 &&
      (overflowY === "auto" ||
        overflowY === "scroll" ||
        overflowY === "overlay")
    ) {
      return element;
    }
    element = element.parentElement ?? undefined;
  }
  return undefined;
}

function nestedScrollerConsumesWheel(
  target: EventTarget | null,
  root: HTMLElement,
  deltaY: number,
): boolean {
  let element = target instanceof HTMLElement ? target : undefined;
  while (element && element !== root) {
    const style = window.getComputedStyle(element);
    const scrollable =
      element.scrollHeight > element.clientHeight + 1 &&
      (style.overflowY === "auto" ||
        style.overflowY === "scroll" ||
        style.overflowY === "overlay");
    if (scrollable) {
      if (
        style.overscrollBehaviorY === "contain" ||
        style.overscrollBehaviorY === "none"
      ) {
        return true;
      }
      if (deltaY < 0 && element.scrollTop > 1) return true;
      if (
        deltaY > 0 &&
        element.scrollTop + element.clientHeight < element.scrollHeight - 1
      ) {
        return true;
      }
    }
    element = element.parentElement ?? undefined;
  }
  return false;
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
  const [messageFileChanges, setMessageFileChanges] = useState<
    Record<string, Record<string, readonly TurnFileChange[]>>
  >({});
  const [plans, setPlans] = useState<Record<string, PlanData | null>>({});
  const [goals, setGoals] = useState<Record<string, GoalSnapshot | null>>({});
  const [goalModeBySession, setGoalModeBySession] = useState<
    Record<string, boolean>
  >({});
  const [swarmModeBySession, setSwarmModeBySession] = useState<
    Record<string, boolean>
  >({});
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
  const [goalEditTarget, setGoalEditTarget] = useState<GoalSnapshot>();
  const [goalEditBusy, setGoalEditBusy] = useState(false);
  const [undoMessageTarget, setUndoMessageTarget] = useState<RenderMessage>();
  const [undoMessageBusy, setUndoMessageBusy] = useState(false);
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
  const chatScrollUpIntentRef = useRef(false);
  const chatScrollIntentFrameRef = useRef<number | undefined>(undefined);
  const chatDisclosureReflowRef = useRef(false);
  const chatDisclosureTimerRef = useRef<number | undefined>(undefined);
  const chatPointerScrollingRef = useRef(false);
  const chatPointerStartRef = useRef<{
    pointerId: number;
    clientX: number;
    clientY: number;
  } | undefined>(undefined);
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
  const latestHistoryUserMessage = useMemo(
    () =>
      [...historyConversationTurns]
        .reverse()
        .find((turn) => turn.user !== undefined)?.user,
    [historyConversationTurns],
  );
  const undoableUserMessageId =
    activeTurn === undefined &&
    latestHistoryUserMessage &&
    isDirectUserMessage(latestHistoryUserMessage)
      ? latestHistoryUserMessage.id
      : undefined;
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
  const activeGoal = activeConversation
    ? goals[activeConversation.id]
    : undefined;
  const activeGoalMode = activeConversation
    ? Boolean(goalModeBySession[activeConversation.id])
    : false;
  const activeSwarmMode = activeConversation
    ? Boolean(swarmModeBySession[activeConversation.id])
    : false;
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
    const [plan, goalResult, goalMode, swarmMode, todos, usage, permission] =
      await Promise.all([
        agent.getPlan(),
        agent.getGoal(),
        getSharedGoalMode(scope.sessionId),
        agent.getSwarmMode(),
        agent.getTodos(),
        agent.getUsage(),
        agent.getPermission(),
      ]);
    setPlans((current) => ({ ...current, [scope.sessionId]: plan }));
    setGoals((current) => ({
      ...current,
      [scope.sessionId]: goalResult.goal,
    }));
    setGoalModeBySession((current) => ({
      ...current,
      [scope.sessionId]: goalMode,
    }));
    setSwarmModeBySession((current) => ({
      ...current,
      [scope.sessionId]: swarmMode,
    }));
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
    setGoalEditTarget(undefined);
    setGoalEditBusy(false);
    setUndoMessageTarget(undefined);
    setUndoMessageBusy(false);
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
    const refreshConversationAfterUndo = async (
      conversationId: string,
    ): Promise<void> => {
      const request = (historyRequests.current[conversationId] ?? 0) + 1;
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
        setInFlightTurns((current) => {
          const turn = current[conversationId];
          if (!turn || isTurnRunning(turn)) return current;
          const next = { ...current };
          delete next[conversationId];
          inFlightTurnsRef.current = next;
          return next;
        });
        setUndoMessageTarget((current) =>
          current?.session_id === conversationId ? undefined : current,
        );
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
    const unlistenGoalModeChanged = listen<GoalModeChangedEvent>(
      "goal-mode-changed",
      (event) => {
        const { sessionId, enabled } = event.payload;
        setGoalModeBySession((current) => ({
          ...current,
          [sessionId]: enabled,
        }));
      },
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
        if (
          isMainAgentEvent &&
          payload.event.type === "conversation.undone"
        ) {
          void refreshConversationAfterUndo(payload.sessionId);
        }
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
        if (isMainAgentEvent && payload.event.type === "goal.updated") {
          const snapshot = payload.event.snapshot;
          if (
            snapshot === null ||
            (typeof snapshot === "object" &&
              snapshot !== null &&
              typeof (snapshot as { objective?: unknown }).objective ===
                "string")
          ) {
            setGoals((current) => ({
              ...current,
              [payload.sessionId]: snapshot as GoalSnapshot | null,
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
        if (
          payload.event.type === "agent.status.updated" &&
          isMainAgentEvent &&
          typeof payload.event.swarmMode === "boolean"
        ) {
          setSwarmModeBySession((current) => ({
            ...current,
            [payload.sessionId]: payload.event.swarmMode as boolean,
          }));
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
      void unlistenGoalModeChanged.then((unlisten) => unlisten());
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
      if (chatScrollIntentFrameRef.current !== undefined) {
        window.cancelAnimationFrame(chatScrollIntentFrameRef.current);
      }
      if (chatDisclosureTimerRef.current !== undefined) {
        window.clearTimeout(chatDisclosureTimerRef.current);
      }
      scrollRef.current?.style.removeProperty("overflow-anchor");
    },
    [],
  );

  const markChatScrollUpIntent = useCallback((): void => {
    chatScrollUpIntentRef.current = true;
    if (chatScrollIntentFrameRef.current !== undefined) {
      window.cancelAnimationFrame(chatScrollIntentFrameRef.current);
    }
    chatScrollIntentFrameRef.current = window.requestAnimationFrame(() => {
      chatScrollIntentFrameRef.current = undefined;
      chatScrollUpIntentRef.current = false;
    });
  }, []);

  useEffect(() => {
    const stopPointerScrolling = (): void => {
      chatPointerScrollingRef.current = false;
      chatPointerStartRef.current = undefined;
    };
    const detectPointerScrolling = (event: PointerEvent): void => {
      const start = chatPointerStartRef.current;
      if (!start || start.pointerId !== event.pointerId) return;
      if (
        Math.abs(event.clientX - start.clientX) > 2 ||
        Math.abs(event.clientY - start.clientY) > 2
      ) {
        chatPointerScrollingRef.current = true;
        if (event.pointerType === "touch" && event.clientY > start.clientY) {
          markChatScrollUpIntent();
        }
        start.clientX = event.clientX;
        start.clientY = event.clientY;
      }
    };
    window.addEventListener("pointermove", detectPointerScrolling);
    window.addEventListener("pointerup", stopPointerScrolling);
    window.addEventListener("pointercancel", stopPointerScrolling);
    window.addEventListener("blur", stopPointerScrolling);
    return () => {
      window.removeEventListener("pointermove", detectPointerScrolling);
      window.removeEventListener("pointerup", stopPointerScrolling);
      window.removeEventListener("pointercancel", stopPointerScrolling);
      window.removeEventListener("blur", stopPointerScrolling);
    };
  }, [markChatScrollUpIntent]);

  // lastChatScrollHeightRef is only refreshed when the view is pinned to the
  // bottom (or on conversation switch), never here. This prevents a content
  // reflow between an append and the next outer pin from looking like the user
  // deliberately scrolled away.
  const handleChatScroll = (event: ReactUIEvent<HTMLDivElement>): void => {
    // Exclude any descendant scroll event delivered by the WebView or React.
    if (event.target !== event.currentTarget) return;
    const scroll = event.currentTarget;
    scheduleActiveOutlineTurnUpdate();
    const distanceFromBottom =
      scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight;
    const contentHeightChanged =
      Math.abs(scroll.scrollHeight - lastChatScrollHeightRef.current) > 1;
    const scrollingUp = scroll.scrollTop < lastChatScrollTopRef.current - 1;
    lastChatScrollTopRef.current = scroll.scrollTop;
    followLatestMessageRef.current = resolveChatFollowState({
      currentlyFollowing: followLatestMessageRef.current,
      distanceFromBottom,
      contentHeightChanged,
      scrollingUp,
      userScrollingUp:
        chatScrollUpIntentRef.current || chatPointerScrollingRef.current,
      userTogglingDisclosure: chatDisclosureReflowRef.current,
    });
  };

  const handleChatDisclosureClick = (
    event: MouseEvent<HTMLDivElement>,
  ): void => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const disclosure = target.closest("button[aria-expanded]");
    if (!disclosure || !event.currentTarget.contains(disclosure)) return;

    const scroll = event.currentTarget;
    const wasFollowing = followLatestMessageRef.current;
    followLatestMessageRef.current = false;
    chatDisclosureReflowRef.current = true;
    scroll.style.overflowAnchor = "none";
    lastChatScrollTopRef.current = scroll.scrollTop;
    lastChatScrollHeightRef.current = scroll.scrollHeight;
    if (chatDisclosureTimerRef.current !== undefined) {
      window.clearTimeout(chatDisclosureTimerRef.current);
    }
    chatDisclosureTimerRef.current = window.setTimeout(() => {
      chatDisclosureTimerRef.current = undefined;
      chatDisclosureReflowRef.current = false;
      scroll.style.removeProperty("overflow-anchor");
      lastChatScrollTopRef.current = scroll.scrollTop;
      lastChatScrollHeightRef.current = scroll.scrollHeight;
      const distanceFromBottom =
        scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight;
      followLatestMessageRef.current =
        wasFollowing && distanceFromBottom <= 48;
    }, 240);
  };

  const handleChatWheel = (event: WheelEvent<HTMLDivElement>): void => {
    if (
      event.deltaY < 0 &&
      !nestedScrollerConsumesWheel(
        event.target,
        event.currentTarget,
        event.deltaY,
      )
    ) {
      markChatScrollUpIntent();
    }
  };

  const handleChatPointerDown = (
    event: ReactPointerEvent<HTMLDivElement>,
  ): void => {
    if (!event.isPrimary || event.button !== 0) return;
    chatPointerScrollingRef.current = false;
    chatPointerStartRef.current = nestedVerticalScroller(
      event.target,
      event.currentTarget,
    )
      ? undefined
      : {
          pointerId: event.pointerId,
          clientX: event.clientX,
          clientY: event.clientY,
        };
  };

  const handleChatKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    if (!isUpwardChatScrollKey(event.key, event.shiftKey)) return;
    const target = event.target;
    if (
      target instanceof HTMLElement &&
      (target.isContentEditable ||
        target.matches("input, textarea, select") ||
        (event.key === " " && target.matches("button")) ||
        nestedVerticalScroller(target, event.currentTarget))
    ) {
      return;
    }
    markChatScrollUpIntent();
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
    setGoals((current) => omitSessionKeys(current, ids));
    setGoalModeBySession((current) => omitSessionKeys(current, ids));
    setSwarmModeBySession((current) => omitSessionKeys(current, ids));
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

  const setSynchronizedGoalMode = async (
    conversationId: string,
    enabled: boolean,
  ): Promise<void> => {
    setGoalModeBySession((current) => ({
      ...current,
      [conversationId]: enabled,
    }));
    try {
      await setSharedGoalMode(conversationId, enabled);
    } catch (error) {
      showNotice(conciseError(error));
    }
  };

  const toggleGoalMode = async (): Promise<void> => {
    if (!activeConversation || !activeAgentScope || activeGoal || modeBusy) {
      return;
    }
    const conversationId = activeConversation.id;
    setModeBusy(true);
    try {
      await setSynchronizedGoalMode(conversationId, !activeGoalMode);
    } finally {
      setModeBusy(false);
    }
  };

  const toggleSwarmMode = async (): Promise<void> => {
    if (!activeConversation || !activeAgentScope || modeBusy || isStreaming) {
      return;
    }
    const conversationId = activeConversation.id;
    const nextMode = !activeSwarmMode;
    setModeBusy(true);
    try {
      const agent = createAgentClient(activeAgentScope);
      if (nextMode) {
        await agent.enterSwarm("manual");
      } else {
        await agent.exitSwarm();
      }
      const enabled = await agent.getSwarmMode();
      setSwarmModeBySession((current) => ({
        ...current,
        [conversationId]: enabled,
      }));
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setModeBusy(false);
    }
  };

  const controlActiveGoal = async (
    action: "pause" | "resume" | "cancel",
  ): Promise<void> => {
    if (!activeConversation || !activeAgentScope || !activeGoal || modeBusy) {
      return;
    }
    const conversationId = activeConversation.id;
    setModeBusy(true);
    try {
      const agent = createAgentClient(activeAgentScope);
      const goal =
        action === "pause"
          ? await agent.pauseGoal()
          : action === "resume"
            ? await agent.resumeGoal()
            : await agent.cancelGoal();
      setGoals((current) => ({
        ...current,
        [conversationId]: action === "cancel" ? null : goal,
      }));
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setModeBusy(false);
    }
  };

  const editActiveGoal = async (
    target: GoalSnapshot,
    objective: string,
  ): Promise<void> => {
    const trimmed = objective.trim();
    if (
      !trimmed ||
      !activeConversation ||
      !activeAgentScope ||
      activeGoal?.goalId !== target.goalId
    ) {
      setGoalEditTarget(undefined);
      if (activeGoal?.goalId !== target.goalId) {
        showNotice(t("goal.changedWhileEditing"));
      }
      return;
    }

    const conversationId = activeConversation.id;
    setGoalEditBusy(true);
    try {
      const agent = createAgentClient(activeAgentScope);
      let goal = await agent.createGoal(
        trimmed,
        true,
        target.completionCriterion,
      );
      if (target.status === "paused") {
        goal = await agent.pauseGoal();
      }
      setGoals((current) => ({ ...current, [conversationId]: goal }));
      setGoalEditTarget(undefined);
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setGoalEditBusy(false);
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
      const fileChanges = completedTurn?.fileChanges;
      if (
        completedTurn &&
        (durationMs !== undefined || (fileChanges?.length ?? 0) > 0)
      ) {
        const messageId = completedTurnMessageId(items, completedTurn);
        if (messageId) {
          if (durationMs !== undefined) {
            setMessageDurations((current) => ({
              ...current,
              [conversationId]: {
                ...current[conversationId],
                [messageId]: durationMs,
              },
            }));
          }
          if (fileChanges && fileChanges.length > 0) {
            setMessageFileChanges((current) => ({
              ...current,
              [conversationId]: {
                ...current[conversationId],
                [messageId]: fileChanges,
              },
            }));
          }
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

  const confirmUndoMessage = async (): Promise<void> => {
    const target = undoMessageTarget;
    const conversation = activeConversation;
    const scope = activeAgentScope;
    if (!target || !conversation || !scope || undoMessageBusy) return;
    if (
      scope.sessionId !== conversation.id ||
      target.id !== undoableUserMessageId ||
      inFlightTurnsRef.current[conversation.id] !== undefined
    ) {
      setUndoMessageTarget(undefined);
      showNotice(t("undo.unavailable"));
      return;
    }

    setUndoMessageBusy(true);
    try {
      await createAgentClient(scope).undoHistory(1);
      const projected = projectLiveUserMessage({
        promptId: target.prompt_id ?? target.id,
        userMessageId: target.id,
        createdAt: target.created_at,
        content: target.content,
      });
      const display = parseSkillPromptDisplay(projected.text);
      await refreshHistory(conversation.id);
      resetPrompt(display.text);
      setPromptAttachments(projected.attachments);
      setPromptSkills(
        availableSkills.filter((skill) => display.skills.includes(skill.name)),
      );
      setUndoMessageTarget(undefined);
      showNotice(t("undo.success"));
      window.requestAnimationFrame(() => {
        const textarea = textareaRef.current;
        if (!textarea) return;
        textarea.focus();
        textarea.setSelectionRange(display.text.length, display.text.length);
      });
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setUndoMessageBusy(false);
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
    queuedGoalMode?: boolean,
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
    const shouldCreateGoal = queuedGoalMode ?? activeGoalMode;
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
    if (shouldCreateGoal && !text) {
      showNotice(t("goal.objectiveRequired"));
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
        goalMode: shouldCreateGoal,
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
      if (shouldCreateGoal) {
        await setSynchronizedGoalMode(conversationId, false);
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

    if (shouldCreateGoal) {
      try {
        const goal = await createAgentClient(activeAgentScope).createGoal(text);
        setGoals((current) => ({ ...current, [conversationId]: goal }));
        await setSynchronizedGoalMode(conversationId, false);
      } catch (error) {
        showNotice(conciseError(error));
        return;
      }
    }

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
    if (
      !queued ||
      queued.steering ||
      queued.skills.length > 0 ||
      queued.goalMode
    ) {
      return;
    }

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
    void sendPrompt(
      queued.text,
      queued.attachments,
      queued.skills,
      queued.goalMode,
    ).finally(() => {
      drainingQueuedPrompts.current.delete(queued.id);
    });
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
              tabIndex={0}
              onScroll={handleChatScroll}
              onClickCapture={handleChatDisclosureClick}
              onWheel={handleChatWheel}
              onPointerDownCapture={handleChatPointerDown}
              onKeyDownCapture={handleChatKeyDown}
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
                      messageFileChanges={
                        messageFileChanges[activeConversation.id] ?? {}
                      }
                      undoableUserMessageId={undoableUserMessageId}
                      onUndoUserMessage={setUndoMessageTarget}
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
              {activeGoal && activeGoal.status !== "complete" && (
                <div className="composer-goal-status">
                  <section
                    className={`composer-goal-card ${activeGoal.status}`}
                    aria-label={t("goal.current")}
                  >
                    <span className="composer-goal-icon" aria-hidden="true">
                      <Target size={15} />
                    </span>
                    <span className="composer-goal-copy">
                      <span className="composer-goal-heading">
                        <strong>{t("goal.current")}</strong>
                        <small>
                          {activeGoal.status === "active"
                            ? t("goal.statusActive")
                            : activeGoal.status === "paused"
                              ? t("goal.statusPaused")
                              : t("goal.statusBlocked")}
                        </small>
                      </span>
                      <span
                        className="composer-goal-objective"
                        title={activeGoal.objective}
                      >
                        {activeGoal.objective}
                      </span>
                    </span>
                    <span className="composer-goal-actions">
                      <button
                        className="icon-only"
                        type="button"
                        aria-label={t("goal.editAction")}
                        title={t("goal.editAction")}
                        disabled={modeBusy}
                        onClick={() => setGoalEditTarget(activeGoal)}
                      >
                        <SquarePen size={12} />
                      </button>
                      {activeGoal.status === "active" && (
                        <button
                          className="icon-only"
                          type="button"
                          aria-label={t("goal.pauseAction")}
                          title={t("goal.pauseAction")}
                          disabled={modeBusy}
                          onClick={() => void controlActiveGoal("pause")}
                        >
                          <Pause size={12} />
                        </button>
                      )}
                      {(activeGoal.status === "paused" ||
                        activeGoal.status === "blocked") && (
                        <button
                          className="primary icon-only"
                          type="button"
                          aria-label={t("goal.resumeAction")}
                          title={t("goal.resumeAction")}
                          disabled={modeBusy}
                          onClick={() => void controlActiveGoal("resume")}
                        >
                          <Play size={12} />
                        </button>
                      )}
                      <button
                        className="cancel"
                        type="button"
                        aria-label={t("goal.cancel")}
                        title={t("goal.cancel")}
                        disabled={modeBusy}
                        onClick={() => void controlActiveGoal("cancel")}
                      >
                        <X size={12} />
                      </button>
                    </span>
                  </section>
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
                {(
                  activePlan ||
                  activeGoalMode ||
                  activeSwarmMode ||
                  promptSkills.length > 0
                ) && (
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
                    {activeGoalMode && (
                      <span className="prompt-skill-chip prompt-goal-chip">
                        <span className="prompt-skill-open prompt-plan-label">
                          <Target size={13} />
                          <span>{t("goal.label")}</span>
                        </span>
                        <button
                          className="prompt-skill-remove"
                          type="button"
                          aria-label={t("goal.disable")}
                          title={t("goal.disable")}
                          disabled={modeBusy}
                          onClick={() => void toggleGoalMode()}
                        >
                          {modeBusy ? (
                            <span className="spinner" />
                          ) : (
                            <X size={11} />
                          )}
                        </button>
                      </span>
                    )}
                    {activeSwarmMode && (
                      <span className="prompt-skill-chip prompt-plan-chip">
                        <span className="prompt-skill-open prompt-plan-label">
                          <RemixSparklingLineIcon size={13} />
                          <span>{t("swarm.label")}</span>
                        </span>
                        <button
                          className="prompt-skill-remove"
                          type="button"
                          aria-label={t("swarm.disable")}
                          title={t("swarm.disable")}
                          disabled={modeBusy || isStreaming}
                          onClick={() => void toggleSwarmMode()}
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
                      : activeGoalMode
                        ? t("composer.placeholderGoal")
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
                            <button
                              className={`composer-add-item ${
                                activeGoal || activeGoalMode ? "selected" : ""
                              }`}
                              type="button"
                              role="menuitemcheckbox"
                              aria-checked={Boolean(
                                activeGoal || activeGoalMode,
                              )}
                              disabled={
                                !activeAgentScope ||
                                modeBusy ||
                                activeGoal?.status === "complete"
                              }
                              onClick={() => {
                                setComposerAddOpen(false);
                                if (activeGoal?.status === "active") {
                                  void controlActiveGoal("pause");
                                } else if (
                                  activeGoal?.status === "paused" ||
                                  activeGoal?.status === "blocked"
                                ) {
                                  void controlActiveGoal("resume");
                                } else {
                                  void toggleGoalMode();
                                }
                              }}
                            >
                              <Target size={15} />
                              <span>
                                <strong>{t("goal.label")}</strong>
                                <small>
                                  {activeGoal?.status === "active"
                                    ? t("goal.pauseDesc")
                                    : activeGoal?.status === "paused" ||
                                        activeGoal?.status === "blocked"
                                      ? t("goal.resumeDesc")
                                      : activeGoal?.objective ?? t("goal.desc")}
                                </small>
                              </span>
                              {(activeGoal || activeGoalMode) && (
                                <Check size={14} />
                              )}
                            </button>
                            <button
                              className={`composer-add-item ${
                                activeSwarmMode ? "selected" : ""
                              }`}
                              type="button"
                              role="menuitemcheckbox"
                              aria-checked={activeSwarmMode}
                              disabled={
                                !activeAgentScope || modeBusy || isStreaming
                              }
                              onClick={() => {
                                setComposerAddOpen(false);
                                void toggleSwarmMode();
                              }}
                            >
                              <RemixSparklingLineIcon size={15} />
                              <span>
                                <strong>{t("swarm.label")}</strong>
                                <small>
                                  {activeSwarmMode
                                    ? t("swarm.disableDesc")
                                    : t("swarm.desc")}
                                </small>
                              </span>
                              {activeSwarmMode && <Check size={14} />}
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

      {undoMessageTarget && (
        <UndoMessageDialog
          busy={undoMessageBusy}
          onClose={() =>
            !undoMessageBusy && setUndoMessageTarget(undefined)
          }
          onConfirm={() => void confirmUndoMessage()}
        />
      )}

      {goalEditTarget && (
        <GoalEditDialog
          goal={goalEditTarget}
          busy={goalEditBusy}
          onClose={() => !goalEditBusy && setGoalEditTarget(undefined)}
          onConfirm={(objective) =>
            void editActiveGoal(goalEditTarget, objective)
          }
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
