import {
  Bot,
  CalendarClock,
  Folder,
  SquarePen
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type FormEvent,
  type KeyboardEvent
} from "react";
import {
  createAgentClient,
  createOrTouchWorkspace,
  forkSession,
  getSharedGoalMode,
  listWorkspaceSessions,
  prepareSession,
  subscribeAgentEvents,
  unsubscribeAgentEvents,
  type McpServerInfo,
  type PluginCommandDef,
} from "./agentRpc";
import { subscribeToAppEvents } from "./app/appEventSubscriptions";
import {
  accountProfileFromUserInfo,
  clearCachedAccountProfiles,
  readCachedAccountProfile,
  writeCachedAccountProfile,
} from "./accountProfileCache";
import {
  BACKGROUND_TASK_LIST_LIMIT,
  BACKGROUND_TASK_OUTPUT_TAIL,
  LIVE_TURN_HANDOFF_MS,
  fetchConversationHistory,
  newQueuedPromptId,
  type AgentSubscription,
  type ConversationHistory,
  type PendingAgentSubscription,
  type SessionSubagentHistories,
} from "./app/appUtils";
import { createWorkspaceActions } from "./app/createWorkspaceActions";
import { useChatScroll } from "./app/useChatScroll";
import { useConversationResources } from "./app/useConversationResources";
import { useResponsiveSidebar } from "./app/useResponsiveSidebar";
import {
  filterPluginCommands,
  parseKnownPluginCommand,
  pluginCommandLabel,
  type SlashMenuItem,
} from "./plugins";
import {
  CUSTOM_FONT_NAME_MAX_LENGTH,
  applyColorScheme,
  applyCustomColors,
  applyCustomFonts,
  applyFontSize,
  loadColorScheme,
  loadCustomColors,
  loadCustomFonts,
  loadFontSize,
  saveColorScheme,
  saveCustomColors,
  saveCustomFonts,
  saveFontSize,
  type ColorScheme,
  type CustomColorKey,
  type CustomColorsByScheme,
  type CustomFonts,
  type FontFamilyPreset,
  type FontRole,
  type FontSize,
} from "./appearance";
import {
  completedTurnMessageId,
  finalResponseMessage,
  groupHistoryMessages,
  historyBeforeInFlightTurn,
  isDirectUserMessage,
  isVisibleHistoryMessage,
  mergeHistoryToolResults,
  messageOriginKind,
  type RenderMessage
} from "./chat/history";
import {
  compactionSummaryForLiveTurn,
  type LiveCompactionEvent,
} from "./chat/conversationTimeline";
import {
  isTurnRunning,
  liveTurnStatusFromSubmit,
  newInFlightTurn,
  type InFlightTurn,
  type PromptAttachment,
  type QueuedAgentChatEvent,
  type QueuedPrompt,
  type RemoteQueuedPrompt,
  type SubagentLiveTurns
} from "./chat/liveTurns";
import {
  displayMessageText,
  messageText,
} from "./chat/messages";
import {
  type RemovalTarget
} from "./components/AppDialogs";
import { AccountAvatar } from "./components/AccountAvatar";
import { AppOverlays } from "./components/AppOverlays";
import { AppSidebar } from "./components/AppSidebar";
import {
  ConversationOutline,
  compactOutlineText,
  conversationOutlinePreview,
  outlineTickWidth,
  type ConversationOutlineItem,
} from "./components/chat/ConversationOutline";
import {
  HistoryTurnView,
  LiveTurnView,
  QueuedPromptList,
  Welcome,
} from "./components/chat/ConversationViews";
import {
  ChatHeaderTitle,
  CompactionNotice,
  WindowTitleBar
} from "./components/ChatHeader";
import { ComposerDock } from "./components/ComposerDock";
import {
  ProjectConversationEmpty,
  ProjectLanding,
} from "./components/ProjectLanding";
import {
  CompactionSummarySidebar,
  PluginCommandDetailSidebar,
  SideChatSidebar,
  SkillDetailSidebar,
  type CompactionSummaryDetail,
  type SideChatState,
  type SkillDetailTarget,
} from "./components/sidebars/ChatSidebars";
import type { PluginCommandDetail } from "./pluginCommandMessage";
import { mergeDesktopInventory } from "./desktopInventory";
import {
  applyLanguage,
  loadLanguage,
  saveLanguage,
  setLanguage,
  t,
  type Language,
} from "./i18n";
import {
  normalizeThinkingLevel,
  thinkingLevelsForModel
} from "./modelControls";
import {
  ensureNotificationPermission,
  listenForNotificationActions,
  loadNotificationsEnabled,
  saveNotificationsEnabled,
  sendConversationNotification,
  shouldNotifyConversation,
  type ConversationNotification,
} from "./notifications";
import {
  buildAgentPromptInput
} from "./prompt/attachments";
import { buildSkillPromptText } from "./prompt/skills";
import {
  promptDraftFor,
  updatePromptDraft,
  type PromptDraftUpdater,
  type PromptDrafts
} from "./promptDrafts";
import {
  canUndoPromptEdit,
  createPromptUndoHistory,
  recordPromptInput,
  undoPromptEdit,
  type PromptUndoHistory,
} from "./promptUndo";
import {
  conversationFromSession,
  conversationFromSummary,
  getActive,
  loadDesktopState
} from "./store";
import {
  collectHistoricalSubagentRuns,
  type SessionSubagentRuns
} from "./subagentEvents";
import { cronTaskBadge, type CronTaskDescriptor } from "./cronTasks";
import {
  loadAutoConversationTitlesEnabled,
  loadConversationTitleModel,
  saveAutoConversationTitlesEnabled,
  saveConversationTitleModel,
} from "./conversationTitles";
import {
  getAppVersion,
  invoke,
  setWebCredential,
  webCredentialRequired
} from "./transport";
import type {
  AccountProfile,
  AccountUsage,
  AgentInteraction,
  AgentUsageStatus,
  AuthStatus,
  BackgroundTaskView,
  ContextUsage,
  DesktopState,
  DeviceCode,
  GoalSnapshot,
  ManagedUserInfo,
  Model,
  PlanData,
  ProtocolMessage,
  SkillContent,
  SkillDescriptor,
  TodoItem
} from "./types";
import { conciseError } from "./utils/errors";

export default function App() {
  const [desktop, setDesktop] = useState<DesktopState>({ projects: [] });
  const [auth, setAuth] = useState<AuthStatus>({
    loggedIn: false,
    provider: "kimi-code",
  });
  const [models, setModels] = useState<Model[]>([]);
  const [promptDrafts, setPromptDrafts] = useState<
    PromptDrafts<PromptAttachment, SkillDescriptor>
  >({});
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
  const [pluginCommandDetail, setPluginCommandDetail] =
    useState<PluginCommandDetail>();
  const [sideChat, setSideChat] = useState<SideChatState>();
  const [composerAddOpen, setComposerAddOpen] = useState(false);
  const [slashMenuOpen, setSlashMenuOpen] = useState(false);
  const [slashMenuActiveIndex, setSlashMenuActiveIndex] = useState(0);
  const [pluginCommands, setPluginCommands] = useState<PluginCommandDef[]>([]);
  const [pluginCommandRevision, setPluginCommandRevision] = useState(0);
  const [mcpStatusOpen, setMcpStatusOpen] = useState(false);
  const [mcpStatusBusy, setMcpStatusBusy] = useState(false);
  const [mcpStatusError, setMcpStatusError] = useState<string>();
  const [mcpServers, setMcpServers] = useState<McpServerInfo[]>([]);
  const [compactionCommandBusy, setCompactionCommandBusy] = useState(false);
  const [forkCommandBusy, setForkCommandBusy] = useState(false);
  const [queuedPrompts, setQueuedPrompts] = useState<
    Record<string, QueuedPrompt[]>
  >({});
  const [remoteQueuedPrompts, setRemoteQueuedPrompts] = useState<
    Record<string, RemoteQueuedPrompt[]>
  >({});
  const [loginOpen, setLoginOpen] = useState(false);
  const [webAuthOpen, setWebAuthOpen] = useState(webCredentialRequired);
  const [directoryPickerOpen, setDirectoryPickerOpen] = useState(false);
  const [loginBusy, setLoginBusy] = useState(false);
  const [deviceCode, setDeviceCode] = useState<DeviceCode>();
  const [profileOpen, setProfileOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [agentManagerOpen, setAgentManagerOpen] = useState(false);
  const [cronManagerOpen, setCronManagerOpen] = useState(false);
  const [cronTaskCounts, setCronTaskCounts] = useState<Record<string, number>>({});
  const [colorScheme, setColorScheme] =
    useState<ColorScheme>(loadColorScheme);
  const [fontSize, setFontSize] = useState<FontSize>(loadFontSize);
  const [customColors, setCustomColors] =
    useState<CustomColorsByScheme>(loadCustomColors);
  const [customFonts, setCustomFonts] = useState<CustomFonts>(loadCustomFonts);
  const [language, setLanguageState] = useState<Language>(loadLanguage);
  const [notificationsEnabled, setNotificationsEnabled] = useState(
    loadNotificationsEnabled,
  );
  const [autoConversationTitlesEnabled, setAutoConversationTitlesEnabled] =
    useState(loadAutoConversationTitlesEnabled);
  const [conversationTitleModel, setConversationTitleModel] = useState(
    loadConversationTitleModel,
  );
  const [appVersion, setAppVersion] = useState<string>();
  const [accountUsage, setAccountUsage] = useState<AccountUsage>();
  const [accountUsageBusy, setAccountUsageBusy] = useState(false);
  const [accountUsageError, setAccountUsageError] = useState<string>();
  const [accountProfile, setAccountProfile] = useState<AccountProfile>();
  const [modelsBusy, setModelsBusy] = useState(false);
  const [modelBusy, setModelBusy] = useState(false);
  const [notice, setNotice] = useState<string>();
  const [copiedMessage, setCopiedMessage] = useState<string>();
  const [interactions, setInteractions] = useState<
    Record<string, AgentInteraction[]>
  >({});
  const [unreadCompletedConversations, setUnreadCompletedConversations] =
    useState<Record<string, true>>({});
  const [resolvingInteraction, setResolvingInteraction] = useState<string>();
  const [compactions, setCompactions] = useState<
    Record<string, LiveCompactionEvent>
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
  const [subagentHistories, setSubagentHistories] =
    useState<SessionSubagentHistories>({});
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
  const [inFlightTurns, setInFlightTurns] = useState<
    Record<string, InFlightTurn>
  >({});
  const inFlightTurnsRef = useRef(inFlightTurns);
  const queuedPromptsRef = useRef(queuedPrompts);
  const desktopRef = useRef(desktop);
  const notificationsEnabledRef = useRef(notificationsEnabled);
  const [activeAgentScope, setActiveAgentScope] = useState<{
    sessionId: string;
    agentId: string;
  }>();
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const promptUndoHistoriesRef = useRef<Record<string, PromptUndoHistory>>({});
  const activeConversationIdRef = useRef<string | undefined>(undefined);
  const promptCompositionRef = useRef(false);
  const attachmentInputRef = useRef<HTMLInputElement>(null);
  const composerAddRef = useRef<HTMLDivElement>(null);
  const profileRef = useRef<HTMLDivElement>(null);
  const noticeTimer = useRef<number | undefined>(undefined);
  const accountUsageRequest = useRef(0);
  const accountProfileRequest = useRef(0);
  const historyRequests = useRef<Record<string, number>>({});
  const subagentHistoryRequests = useRef<Record<string, number>>({});
  const subagentHistoryPromises = useRef<Map<string, Promise<void>>>(new Map());
  const observedHistoryItems = useRef<Record<string, ProtocolMessage[]>>({});
  const desktopInventoryRequest = useRef(0);
  const backgroundTaskRequests = useRef<Record<string, number>>({});
  const skillsRequest = useRef(0);
  const skillDetailRequest = useRef(0);
  const mcpStatusRequest = useRef(0);
  const agentSubscriptions = useRef<Map<string, AgentSubscription>>(new Map());
  const pendingAgentSubscriptions = useRef<
    Map<string, PendingAgentSubscription>
  >(new Map());
  const queuedAgentChatEvents = useRef<QueuedAgentChatEvent[]>([]);
  const agentChatEventFrame = useRef<number | undefined>(undefined);
  const drainingQueuedPrompts = useRef(new Set<string>());
  const cronTurnRunningRef = useRef<Record<string, boolean>>({});
  const sideChatInstance = useRef(0);
  const sideChatAgentId = useRef<string | undefined>(undefined);
  const sideChatAgentIds = useRef(new Set<string>());

  const {
    desktopRuntime,
    mobileLayout,
    mobileSidebarOpen,
    mobileViewportHeight,
    sidebarCollapsed,
    mobileMenuButtonRef,
    closeMobileNavigation,
    openSidebar,
    toggleSidebar,
    expandDesktopSidebar,
  } = useResponsiveSidebar(setProfileOpen);


  const { project: activeProject, conversation: activeConversation } = useMemo(
    () => getActive(desktop),
    [desktop],
  );
  activeConversationIdRef.current = activeConversation?.id;
  desktopRef.current = desktop;
  notificationsEnabledRef.current = notificationsEnabled;
  useEffect(() => {
    if (!activeProject) return;
    void createOrTouchWorkspace(activeProject.path).catch(() => {
      // Remembering the active workspace is best-effort and must not block navigation.
    });
  }, [activeProject?.id, activeProject?.path]);
  const activePromptDraft = promptDraftFor(
    promptDrafts,
    activeConversation?.id,
  );
  const prompt = activePromptDraft.text;
  const promptAttachments = activePromptDraft.attachments;
  const promptSkills = activePromptDraft.skills;
  const setPromptText = (
    update: PromptDraftUpdater<string>,
    conversationId = activeConversation?.id,
  ): void => {
    if (!conversationId) return;
    setPromptDrafts((current) =>
      updatePromptDraft(current, conversationId, "text", update),
    );
  };
  const setPromptAttachments = (
    update: PromptDraftUpdater<PromptAttachment[]>,
    conversationId = activeConversation?.id,
  ): void => {
    if (!conversationId) return;
    setPromptDrafts((current) =>
      updatePromptDraft(current, conversationId, "attachments", update),
    );
  };
  const setPromptSkills = (
    update: PromptDraftUpdater<SkillDescriptor[]>,
    conversationId = activeConversation?.id,
  ): void => {
    if (!conversationId) return;
    setPromptDrafts((current) =>
      updatePromptDraft(current, conversationId, "skills", update),
    );
  };
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
  const activeSubagentHistories = activeConversation
    ? subagentHistories[activeConversation.id]
    : undefined;
  const activeHistory = activeConversation
    ? historyByConversation[activeConversation.id]
    : undefined;
  const activeCronTaskCount = activeConversation
    ? cronTaskCounts[activeConversation.id] ?? 0
    : 0;

  const loadSubagentHistory = useCallback((
    sessionId: string,
    agentId: string,
    force = false,
  ): void => {
    const key = `${sessionId}\u0000${agentId}`;
    if (!force && subagentHistoryPromises.current.has(key)) return;
    const cached = subagentHistories[sessionId]?.[agentId];
    if (!force && cached && !cached.loading && !cached.error) return;

    const request = (subagentHistoryRequests.current[key] ?? 0) + 1;
    subagentHistoryRequests.current[key] = request;
    setSubagentHistories((current) => ({
      ...current,
      [sessionId]: {
        ...current[sessionId],
        [agentId]: {
          agentId,
          items: current[sessionId]?.[agentId]?.items ?? [],
          loading: true,
        },
      },
    }));

    const pending = fetchConversationHistory(sessionId, agentId)
      .then((page) => {
        if (request !== subagentHistoryRequests.current[key]) return;
        setSubagentHistories((current) => ({
          ...current,
          [sessionId]: {
            ...current[sessionId],
            [agentId]: {
              agentId,
              items: page.items,
              loading: false,
            },
          },
        }));
      })
      .catch((error) => {
        if (request !== subagentHistoryRequests.current[key]) return;
        setSubagentHistories((current) => ({
          ...current,
          [sessionId]: {
            ...current[sessionId],
            [agentId]: {
              agentId,
              items: current[sessionId]?.[agentId]?.items ?? [],
              loading: false,
              error: conciseError(error),
            },
          },
        }));
      })
      .finally(() => {
        if (subagentHistoryPromises.current.get(key) === pending) {
          subagentHistoryPromises.current.delete(key);
        }
      });
    subagentHistoryPromises.current.set(key, pending);
  }, [subagentHistories]);

  useEffect(() => {
    const changedSessions: string[] = [];
    for (const [sessionId, history] of Object.entries(historyByConversation)) {
      const previous = observedHistoryItems.current[sessionId];
      if (previous && previous !== history.items) changedSessions.push(sessionId);
      observedHistoryItems.current[sessionId] = history.items;
    }
    if (changedSessions.length === 0) return;
    const changed = new Set(changedSessions);
    for (const key of Object.keys(subagentHistoryRequests.current)) {
      const sessionId = key.slice(0, key.indexOf("\u0000"));
      if (changed.has(sessionId)) {
        subagentHistoryRequests.current[key] += 1;
        subagentHistoryPromises.current.delete(key);
      }
    }
    setSubagentHistories((current) => {
      let next = current;
      for (const sessionId of changed) {
        if (!(sessionId in next)) continue;
        if (next === current) next = { ...current };
        delete next[sessionId];
      }
      return next;
    });
  }, [historyByConversation]);

  useEffect(() => {
    const knownSessions = new Set(
      desktop.projects.flatMap((project) =>
        project.conversations.map((conversation) => conversation.id),
      ),
    );
    setSubagentHistories((current) => {
      let changed = false;
      const next: SessionSubagentHistories = {};
      for (const [sessionId, histories] of Object.entries(current)) {
        if (!knownSessions.has(sessionId)) {
          changed = true;
          continue;
        }
        next[sessionId] = histories;
      }
      return changed ? next : current;
    });
    for (const key of Object.keys(subagentHistoryRequests.current)) {
      const sessionId = key.slice(0, key.indexOf("\u0000"));
      if (knownSessions.has(sessionId)) continue;
      subagentHistoryRequests.current[key] += 1;
      subagentHistoryPromises.current.delete(key);
    }
    for (const sessionId of Object.keys(observedHistoryItems.current)) {
      if (!knownSessions.has(sessionId)) {
        delete observedHistoryItems.current[sessionId];
      }
    }
  }, [desktop.projects]);
  const updateCronTaskCount = useCallback((sessionId: string, count: number): void => {
    setCronTaskCounts((current) => ({ ...current, [sessionId]: count }));
  }, []);
  const refreshCronTaskCount = useCallback(
    async (sessionId: string): Promise<void> => {
      const tasks = await invoke<CronTaskDescriptor[]>("list_cron_tasks", { sessionId });
      updateCronTaskCount(sessionId, tasks.length);
    },
    [updateCronTaskCount],
  );

  useEffect(() => {
    inFlightTurnsRef.current = inFlightTurns;
  }, [inFlightTurns]);
  useEffect(() => {
    queuedPromptsRef.current = queuedPrompts;
  }, [queuedPrompts]);
  const pendingHandoffTurns = useMemo(
    () =>
      (activeTurn?.handoffTurns ?? []).filter(
        (turn) =>
          !activeHistory ||
          completedTurnMessageId(activeHistory.items, turn) === undefined,
      ),
    [activeHistory?.items, activeTurn?.handoffTurns],
  );
  const liveHistoryBoundaryTurn = pendingHandoffTurns[0] ?? activeTurn;
  const visibleHistoryMessages = useMemo(
    () =>
      (activeHistory
        ? liveHistoryBoundaryTurn
          ? historyBeforeInFlightTurn(
              activeHistory.items,
              liveHistoryBoundaryTurn,
            )
          : activeHistory.items
        : []
      ).filter(isVisibleHistoryMessage),
    [
      activeHistory?.items,
      liveHistoryBoundaryTurn?.historyBoundaryId,
      liveHistoryBoundaryTurn?.userMessageId,
      liveHistoryBoundaryTurn?.prompt,
    ],
  );
  const historyToolPresentation = useMemo(
    () => mergeHistoryToolResults(visibleHistoryMessages),
    [visibleHistoryMessages],
  );
  const historicalSubagentRuns = useMemo(
    () => collectHistoricalSubagentRuns(visibleHistoryMessages),
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
  const activeLiveCompactionSummary =
    activeCompaction?.phase === "completed" && activeTurn && activeHistory
      ? compactionSummaryForLiveTurn(activeHistory.items, activeTurn)
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
  const canViewMcpStatus =
    activeConversation !== undefined &&
    activeAgentScope?.sessionId === activeConversation.id;
  const slashMenuItems = useMemo<SlashMenuItem[]>(() => {
    const query = prompt.startsWith("/") ? prompt.slice(1).toLowerCase() : "";
    const builtins: SlashMenuItem[] = [
      {
        id: "compact",
        kind: "builtin",
        builtin: "compact",
        label: "compact",
        description:
          activeCompaction?.phase === "started"
            ? t("slash.compacting")
            : activeContextPercent === undefined
              ? t("slash.compactDesc")
              : t("slash.compactDescPercent", { percent: activeContextPercent }),
        disabled: !canRunCompaction,
      },
      {
        id: "fork",
        kind: "builtin",
        builtin: "fork",
        label: "fork",
        description: t("slash.forkDesc"),
        disabled: !canRunFork,
      },
      {
        id: "btw",
        kind: "builtin",
        builtin: "btw",
        label: "btw",
        description: t("slash.sideChatDesc"),
        disabled: !canOpenSideChat,
      },
      {
        id: "mcp",
        kind: "builtin",
        builtin: "mcp",
        label: "mcp",
        description: t("slash.mcpDesc"),
        disabled: !canViewMcpStatus,
      },
    ];
    const visibleBuiltins = builtins.filter((item) =>
      item.label.toLowerCase().includes(query),
    );
    const visiblePlugins = filterPluginCommands(pluginCommands, query).map(
      (command): SlashMenuItem => ({
        id: `plugin-${command.pluginId}-${command.name}`,
        kind: "plugin",
        label: pluginCommandLabel(command),
        description: command.description,
        plugin: command,
      }),
    );
    return [...visibleBuiltins, ...visiblePlugins];
  }, [
    activeCompaction?.phase,
    activeContextPercent,
    canOpenSideChat,
    canViewMcpStatus,
    canRunCompaction,
    canRunFork,
    language,
    pluginCommands,
    prompt,
  ]);
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

  const notifyConversation = (
    notification: Omit<ConversationNotification, "conversationTitle">,
  ): void => {
    if (
      notification.kind === "completed" &&
      notification.sessionId !== activeConversationIdRef.current
    ) {
      setUnreadCompletedConversations((current) => ({
        ...current,
        [notification.sessionId]: true,
      }));
    }
    if (!notificationsEnabledRef.current) return;
    const windowFocused =
      document.hasFocus() && document.visibilityState === "visible";
    if (
      !shouldNotifyConversation(
        notification.sessionId,
        activeConversationIdRef.current,
        windowFocused,
      )
    ) {
      return;
    }
    const conversation = desktopRef.current.projects
      .flatMap((project) => project.conversations)
      .find((item) => item.id === notification.sessionId);
    void sendConversationNotification({
      ...notification,
      conversationTitle: conversation?.title,
    }).catch(() => {
      // Notification delivery failures must not interrupt the active session.
    });
  };

  const openNotificationSession = (sessionId: string): void => {
    const project = desktopRef.current.projects.find((item) =>
      item.conversations.some((conversation) => conversation.id === sessionId),
    );
    if (!project) return;
    updateDesktop((current) => ({
      ...current,
      activeProjectId: project.id,
      activeConversationId: sessionId,
    }));
  };

  const closeSideChat = useCallback((): void => {
    sideChatInstance.current += 1;
    sideChatAgentId.current = undefined;
    setSideChat(undefined);
  }, []);

  const closeMcpStatus = useCallback((): void => {
    mcpStatusRequest.current += 1;
    setMcpStatusOpen(false);
    setMcpStatusBusy(false);
  }, []);

  const openPluginCommandDetail = useCallback(
    (command: PluginCommandDetail): void => {
      closeSideChat();
      skillDetailRequest.current += 1;
      setSkillDetailTarget(undefined);
      setSkillDetail(undefined);
      setSkillDetailBusy(false);
      setSkillDetailError(undefined);
      setCompactionSummaryDetail(undefined);
      setPluginCommandDetail(command);
    },
    [closeSideChat],
  );

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

  const stopBackgroundTask = useCallback(
    async (
      scope: { sessionId: string; agentId: string },
      taskId: string,
    ): Promise<void> => {
      await createAgentClient(scope).stopTask(taskId);
      await refreshBackgroundTasks(scope);
    },
    [refreshBackgroundTasks],
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

  const loadAccountProfile = async (
    provider = auth.provider,
  ): Promise<void> => {
    const request = accountProfileRequest.current + 1;
    accountProfileRequest.current = request;
    try {
      const userInfo = await invoke<ManagedUserInfo>("account_profile");
      if (request === accountProfileRequest.current) {
        const profile = accountProfileFromUserInfo(userInfo);
        setAccountProfile(profile);
        writeCachedAccountProfile(provider, profile);
      }
    } catch {
      // Profile display is best-effort; keep the previous value on failure.
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

  const updateFontSize = (nextFontSize: FontSize): void => {
    setFontSize(nextFontSize);
    saveFontSize(nextFontSize);
  };

  const updateCustomColor = (
    key: CustomColorKey,
    value: string | undefined,
  ): void => {
    setCustomColors((current) => {
      const next: CustomColorsByScheme = {
        ...current,
        [colorScheme]: { ...current[colorScheme], [key]: value },
      };
      if (!value) delete next[colorScheme][key];
      saveCustomColors(next);
      return next;
    });
  };

  const updateCustomFonts = (
    key: FontRole,
    value: FontFamilyPreset,
  ): void => {
    setCustomFonts((current) => {
      const next: CustomFonts = {
        ...current,
        [key]: value === "kimi" ? undefined : value,
      };
      saveCustomFonts(next);
      return next;
    });
  };

  const updateCustomFontName = (role: FontRole, value: string): void => {
    setCustomFonts((current) => {
      const key = role === "sans" ? "sansCustom" : "monoCustom";
      const normalized = value
        .replace(/[\u0000-\u001f,]/g, "")
        .slice(0, CUSTOM_FONT_NAME_MAX_LENGTH);
      const next: CustomFonts = {
        ...current,
        [key]: normalized || undefined,
      };
      saveCustomFonts(next);
      return next;
    });
  };

  const updateLanguage = (nextLanguage: Language): void => {
    setLanguage(nextLanguage);
    setLanguageState(nextLanguage);
    saveLanguage(nextLanguage);
  };

  const updateNotificationsEnabled = async (enabled: boolean): Promise<void> => {
    if (enabled) {
      try {
        if (!(await ensureNotificationPermission(true))) {
          showNotice(t("settings.notificationsPermissionDenied"));
          return;
        }
      } catch {
        showNotice(t("settings.notificationsPermissionDenied"));
        return;
      }
    }
    setNotificationsEnabled(enabled);
    saveNotificationsEnabled(enabled);
  };

  const updateAutoConversationTitlesEnabled = (enabled: boolean): void => {
    setAutoConversationTitlesEnabled(enabled);
    saveAutoConversationTitlesEnabled(enabled);
  };

  const updateConversationTitleModel = (modelId?: string): void => {
    setConversationTitleModel(modelId);
    saveConversationTitleModel(modelId);
  };

  useLayoutEffect(() => {
    applyColorScheme(colorScheme);
  }, [colorScheme]);

  useLayoutEffect(() => {
    applyFontSize(fontSize);
  }, [fontSize]);

  useLayoutEffect(() => {
    applyCustomColors(customColors[colorScheme], colorScheme);
  }, [customColors, colorScheme]);

  useLayoutEffect(() => {
    applyCustomFonts(customFonts);
  }, [customFonts]);

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
    const modelsLoaded = loadModels();
    void invoke<AuthStatus>("auth_status")
      .then((status) => {
        if (!active) return;
        setAuth(status);
        if (status.loggedIn) {
          setAccountProfile(readCachedAccountProfile(status.provider));
          void loadAccountProfile(status.provider);
          void modelsLoaded.then(() => {
            if (active) void refreshModels();
          });
        } else {
          accountProfileRequest.current += 1;
          setAccountProfile(undefined);
          clearCachedAccountProfiles();
        }
      })
      .catch(() => {
        // Vite's browser preview has no Tauri bridge; the actual desktop app does.
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
    const scope = activeAgentScope;
    if (!scope) {
      setPluginCommands([]);
      return;
    }
    let active = true;
    void createAgentClient(scope)
      .listPluginCommands()
      .then((commands) => {
        if (active) setPluginCommands(commands);
      })
      .catch(() => {
        if (active) setPluginCommands([]);
      });
    return () => {
      active = false;
    };
  }, [activeAgentScope?.agentId, activeAgentScope?.sessionId, pluginCommandRevision]);

  useEffect(() => {
    if (slashMenuActiveIndex < slashMenuItems.length) return;
    setSlashMenuActiveIndex(Math.max(0, slashMenuItems.length - 1));
  }, [slashMenuActiveIndex, slashMenuItems.length]);

  useEffect(() => {
    promptCompositionRef.current = false;
    skillsRequest.current += 1;
    skillDetailRequest.current += 1;
    setComposerAddOpen(false);
    setAvailableSkills([]);
    setSkillsBusy(false);
    setSkillsError(undefined);
    setSlashMenuOpen(false);
    closeMcpStatus();
    closeSideChat();
    setCompactionSummaryDetail(undefined);
    setPluginCommandDetail(undefined);
    setSkillDetailTarget(undefined);
    setSkillDetail(undefined);
    setSkillDetailBusy(false);
    setSkillDetailError(undefined);
    setGoalEditTarget(undefined);
    setGoalEditBusy(false);
    setUndoMessageTarget(undefined);
    setUndoMessageBusy(false);
    setCronManagerOpen(false);
  }, [activeConversation?.id, closeMcpStatus, closeSideChat]);

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

  useEffect(() => {
    const sessionId = activeAgentScope?.sessionId;
    if (!sessionId) return;
    void refreshCronTaskCount(sessionId).catch(() => {
      // A later turn completion or opening the manager will retry.
    });
  }, [activeAgentScope?.sessionId, refreshCronTaskCount]);

  useEffect(() => {
    const sessionId = activeConversation?.id;
    if (!sessionId) return;
    const wasRunning = Boolean(cronTurnRunningRef.current[sessionId]);
    const running = isTurnRunning(activeTurn);
    cronTurnRunningRef.current[sessionId] = running;
    if (
      wasRunning &&
      !running &&
      activeAgentScope?.sessionId === sessionId
    ) {
      void refreshCronTaskCount(sessionId).catch(() => {
        // Opening the manager remains an explicit retry path.
      });
    }
  }, [
    activeAgentScope?.sessionId,
    activeConversation?.id,
    activeTurn?.status,
    refreshCronTaskCount,
  ]);

  useEffect(
    () => () => releaseAllAgentSubscriptions(),
    [],
  );

  useEffect(
    () =>
      subscribeToAppEvents({
        agentChatEventFrame,
        desktopInventoryRequest,
        historyRequests,
        inFlightTurnsRef,
        queuedPromptsRef,
        queuedAgentChatEvents,
        sideChatAgentId,
        sideChatAgentIds,
        loadBackgroundTaskOutput,
        refreshBackgroundTasks,
        setAgentUsages,
        setBackgroundTasks,
        setCompactionHistoryReady,
        setCompactions,
        setContextUsages,
        setDesktop,
        setDeviceCode,
        setGoalModeBySession,
        setGoals,
        setHistoryByConversation,
        setInFlightTurns,
        setInteractions,
        setLoginOpen,
        setPlans,
        setQueuedPrompts,
        setRemoteQueuedPrompts,
        setSessionTodos,
        setSideChat,
        setSubagentLiveTurns,
        setSubagentRuns,
        setSwarmModeBySession,
        setUndoMessageTarget,
        setWebAuthOpen,
        notifyConversation,
        showNotice,
        updateDesktop,
      }),
    [],
  );

  useEffect(() => {
    const conversationId = activeConversation?.id;
    if (!conversationId) return;
    setUnreadCompletedConversations((current) => {
      if (!current[conversationId]) return current;
      const next = { ...current };
      delete next[conversationId];
      return next;
    });
  }, [activeConversation?.id]);

  useEffect(() => {
    let disposed = false;
    let listener: Awaited<ReturnType<typeof listenForNotificationActions>>;
    void listenForNotificationActions((sessionId) => {
      openNotificationSession(sessionId);
    })
      .then((registered) => {
        if (disposed) {
          registered?.();
        } else {
          listener = registered;
        }
      })
      .catch(() => {
        // Desktop platforms without notification action events still show alerts.
      });
    return () => {
      disposed = true;
      listener?.();
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
            items: page.items,
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

  const {
    activeOutlineTurnId,
    followLatestMessageRef,
    scrollRef,
    messageStackRef,
    handleChatScroll,
    handleChatDisclosureClick,
    handleChatWheel,
    handleChatPointerDown,
    handleChatKeyDown,
    scrollToConversationTurn,
  } = useChatScroll({
    conversationId: activeConversation?.id,
    historyLoading: activeHistory?.loading,
    hasVisibleMessages,
    outlineItems: conversationOutlineItems,
  });

  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    textarea.style.height = "0px";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 180)}px`;
  }, [prompt]);


  const resetPrompt = (
    value = "",
    conversationId = activeConversation?.id,
  ): void => {
    if (!conversationId) return;
    promptUndoHistoriesRef.current[conversationId] =
      createPromptUndoHistory(value);
    setPromptText(value, conversationId);
    if (activeConversationIdRef.current === conversationId) {
      promptCompositionRef.current = false;
      setSlashMenuOpen(false);
    }
  };

  const updatePrompt = (value: string, isComposing = false): void => {
    const conversationId = activeConversation?.id;
    if (!conversationId) return;
    const current =
      promptUndoHistoriesRef.current[conversationId] ??
      createPromptUndoHistory(prompt);
    promptUndoHistoriesRef.current[conversationId] = recordPromptInput(
      current,
      value,
      { isComposing },
    );
    setPromptText(value, conversationId);
  };

  const syncSlashMenu = (textarea: HTMLTextAreaElement): void => {
    const value = textarea.value;
    const open =
      document.activeElement === textarea &&
      value.startsWith("/") &&
      !/\s/.test(value) &&
      textarea.selectionStart === value.length &&
      textarea.selectionEnd === value.length;
    setSlashMenuOpen(open);
    if (open) {
      setComposerAddOpen(false);
      closeMcpStatus();
    }
  };

  const undoPrompt = (): void => {
    const conversationId = activeConversation?.id;
    if (!conversationId) return;
    const current =
      promptUndoHistoriesRef.current[conversationId] ??
      createPromptUndoHistory(prompt);
    const history = undoPromptEdit(current);
    if (history === current) return;
    promptUndoHistoriesRef.current[conversationId] = history;
    setPromptText(history.current, conversationId);
    window.requestAnimationFrame(() => {
      const textarea = textareaRef.current;
      if (!textarea) return;
      textarea.focus();
      textarea.setSelectionRange(history.current.length, history.current.length);
    });
  };

  const {
    confirmRemoval,
    addProjectPath,
    addProject,
    createConversation,
    selectConversation,
    toggleProject,
    chooseModel,
    choosePermissionMode,
    renameConversation,
    chooseEffort,
    togglePlanMode,
    setSynchronizedGoalMode,
    toggleGoalMode,
    toggleSwarmMode,
    controlActiveGoal,
    editActiveGoal,
  } = createWorkspaceActions({
    activeAgentScope,
    activeConversation,
    activeGoal,
    activeGoalMode,
    activePlan,
    activeProject,
    activeSwarmMode,
    backgroundTaskRequests,
    closeMobileNavigation,
    desktop,
    effort,
    expandDesktopSidebar,
    historyRequests,
    inFlightTurnsRef,
    isStreaming,
    mobileLayout,
    modeBusy,
    modelBusy,
    models,
    permissionMode,
    promptUndoHistoriesRef,
    refreshAgentState,
    releaseAgentSubscription,
    removalBusy,
    removalTarget,
    selectedModel,
    setAgentUsages,
    setBackgroundTasks,
    setCompactionHistoryReady,
    setCompactions,
    setContextUsages,
    setDirectoryPickerOpen,
    setGoalEditBusy,
    setGoalEditTarget,
    setGoalModeBySession,
    setGoals,
    setHistoryByConversation,
    setInFlightTurns,
    setInteractions,
    setMessageDurations,
    setModeBusy,
    setModelBusy,
    setModels,
    setPlans,
    setPromptDrafts,
    setQueuedPrompts,
    setRemoteQueuedPrompts,
    setRemovalBusy,
    setRemovalTarget,
    setResolvingInteraction,
    setSessionTodos,
    setSubagentLiveTurns,
    setSubagentRuns,
    setSwarmModeBySession,
    showNotice,
    updateDesktop,
  });

  const {
    startLogin,
    signOut,
    refreshHistory,
    confirmUndoMessage,
    loadAvailableSkills,
    toggleComposerAdd,
    selectPromptSkill,
    openSkillDetail,
    closeSkillDetail,
    openCompactionSummary,
    handleAttachmentInput,
    handlePromptPaste,
  } = useConversationResources({
    accountProfileRequest,
    accountUsageRequest,
    activeAgentScope,
    activeCompaction,
    activeConversation,
    activeConversationIdRef,
    availableSkills,
    closeSideChat,
    composerAddOpen,
    historyRequests,
    inFlightTurnsRef,
    loadAccountProfile,
    promptAttachments,
    promptSkills,
    refreshModels,
    resetPrompt,
    selectedModel,
    setAccountProfile,
    setAccountUsage,
    setAccountUsageBusy,
    setAccountUsageError,
    setAuth,
    setAvailableSkills,
    setCompactionHistoryReady,
    setCompactionSummaryDetail,
    setPluginCommandDetail,
    setComposerAddOpen,
    setDeviceCode,
    setHistoryByConversation,
    setLoginBusy,
    setLoginOpen,
    setMessageDurations,
    setProfileOpen,
    setPromptAttachments,
    setPromptSkills,
    setSkillDetail,
    setSkillDetailBusy,
    setSkillDetailError,
    setSkillDetailTarget,
    setSkillsBusy,
    setSkillsError,
    setUndoMessageBusy,
    setUndoMessageTarget,
    showNotice,
    skillDetailRequest,
    skillsRequest,
    textareaRef,
    undoableUserMessageId,
    undoMessageBusy,
    undoMessageTarget,
  });

  const sendPrompt = async (
    override?: string,
    queuedAttachments?: readonly PromptAttachment[],
    queuedSkills?: readonly SkillDescriptor[],
    queuedGoalMode?: boolean,
    queuedPromptId?: string,
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
        resetPrompt("", conversationId);
        setPromptAttachments([], conversationId);
        setPromptSkills([], conversationId);
      }
      if (shouldCreateGoal) {
        await setSynchronizedGoalMode(conversationId, false);
      }
      followLatestMessageRef.current = true;
      return;
    }

    const isFirstConversationMessage =
      activeConversation.title === t("conversation.new");
    const title =
      isFirstConversationMessage
        ? (
            text ||
            submittedText ||
            t("conversation.mediaTitle", { count: attachments.length })
          )
            .replace(/\s+/g, " ")
            .slice(0, 28)
        : activeConversation.title;
    const titleSource =
      autoConversationTitlesEnabled &&
      isFirstConversationMessage &&
      text &&
      !queuedPromptId
        ? text
        : undefined;
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
    if (!queuedPromptId) {
      setInFlightTurns((current) => ({
        ...current,
        [conversationId]: newInFlightTurn(
          text,
          attachments,
          activeHistory?.items.at(-1)?.id,
          skills.map((skill) => skill.name),
        ),
      }));
    }
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
      resetPrompt("", conversationId);
      setPromptAttachments([], conversationId);
      setPromptSkills([], conversationId);
    }

    try {
      const client = createAgentClient(activeAgentScope);
      const submitted = await client.prompt(input, {
        promptId: queuedPromptId,
        skills: skills.map((skill) => ({ name: skill.name })),
      });
      if (titleSource) {
        void client
          .generateConversationTitle(titleSource, conversationTitleModel)
          .then((generatedTitle) => {
            if (!generatedTitle) return;
            updateDesktop((current) => ({
              ...current,
              projects: current.projects.map((project) =>
                project.id !== projectId
                  ? project
                  : {
                      ...project,
                      conversations: project.conversations.map((conversation) =>
                        conversation.id === conversationId
                          ? { ...conversation, title: generatedTitle }
                          : conversation,
                      ),
                    },
              ),
            }));
          })
          .catch(() => {
            // Keep the existing first-message title when generation fails.
          });
      }
      if (queuedPromptId) {
        // `prompt.submitted` only means the server owns the message.  Keep the
        // local row visible until `turn.started` proves that execution began.
        setQueuedPrompts((current) => ({
          ...current,
          [conversationId]: (current[conversationId] ?? []).map((item) =>
            item.id === queuedPromptId
              ? { ...item, executionState: "waiting" }
              : item,
          ),
        }));
        return;
      }
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
      if (queuedPromptId) {
        setQueuedPrompts((current) => ({
          ...current,
          [conversationId]: (current[conversationId] ?? []).map((item) =>
            item.id === queuedPromptId
              ? { ...item, executionState: undefined }
              : item,
          ),
        }));
        showNotice(message);
        return;
      }
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
        (item) => item.id !== queuedPromptId || item.executionState,
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
      queued.executionState ||
      queued.skills.length > 0 ||
      queued.goalMode
    ) {
      return;
    }

    setQueuedPrompts((current) => ({
      ...current,
      [conversationId]: (current[conversationId] ?? []).map((item) =>
        item.id === queuedPromptId
          ? { ...item, executionState: "submitting" }
          : item,
      ),
    }));

    try {
      await createAgentClient(activeAgentScope).steer(
        buildAgentPromptInput(
          buildSkillPromptText(queued.text, queued.skills),
          queued.attachments,
        ),
        queued.id,
      );
      setQueuedPrompts((current) => ({
        ...current,
        [conversationId]: (current[conversationId] ?? []).map(
          (item) =>
            item.id === queuedPromptId
              ? { ...item, executionState: "waiting" }
              : item,
        ),
      }));
    } catch (error) {
      setQueuedPrompts((current) => ({
        ...current,
        [conversationId]: (current[conversationId] ?? []).map((item) =>
          item.id === queuedPromptId
            ? { ...item, executionState: undefined }
            : item,
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
      queued.executionState !== undefined ||
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
      [conversationId]: (current[conversationId] ?? []).map(
        (item) =>
          item.id === queued.id
            ? { ...item, executionState: "submitting" }
            : item,
      ),
    }));
    void sendPrompt(
      queued.text,
      queued.attachments,
      queued.skills,
      queued.goalMode,
      queued.id,
    ).finally(() => {
      drainingQueuedPrompts.current.delete(queued.id);
    });
  }, [
    activeAgentScope?.sessionId,
    activeConversation?.id,
    activeQueuedPrompts[0]?.id,
    activeQueuedPrompts[0]?.executionState,
    activeTurn,
    hasBlockingInteraction,
    isHistoryLoading,
    modelBusy,
  ]);

  const executePluginCommand = async (
    command: ReturnType<typeof parseKnownPluginCommand>,
  ): Promise<void> => {
    if (!command) return;
    const scope = activeAgentScope;
    if (!scope || scope.sessionId !== activeConversation?.id) {
      showNotice(t("notice.sessionPreparing"));
      return;
    }
    try {
      await createAgentClient(scope).activatePluginCommand(
        command.pluginId,
        command.commandName,
        command.args,
      );
      resetPrompt("", scope.sessionId);
      setSlashMenuOpen(false);
    } catch (error) {
      showNotice(conciseError(error));
    }
  };

  const openMcpStatus = async (): Promise<void> => {
    const scope = activeAgentScope;
    if (!scope || scope.sessionId !== activeConversation?.id) {
      showNotice(t("notice.sessionPreparing"));
      return;
    }
    const request = mcpStatusRequest.current + 1;
    mcpStatusRequest.current = request;
    setComposerAddOpen(false);
    setSlashMenuOpen(false);
    resetPrompt("", scope.sessionId);
    setMcpStatusOpen(true);
    setMcpStatusBusy(true);
    setMcpStatusError(undefined);
    try {
      const servers = await createAgentClient(scope).listMcpServers();
      if (request !== mcpStatusRequest.current) return;
      setMcpServers(servers);
    } catch (error) {
      if (request !== mcpStatusRequest.current) return;
      setMcpServers([]);
      setMcpStatusError(conciseError(error));
    } finally {
      if (request === mcpStatusRequest.current) setMcpStatusBusy(false);
    }
  };

  const submitComposer = (): void => {
    if (prompt.trim().toLowerCase() === "/mcp") {
      void openMcpStatus();
      return;
    }
    const command = parseKnownPluginCommand(prompt, pluginCommands);
    if (command) {
      if (promptAttachments.length > 0 || promptSkills.length > 0) {
        showNotice(t("plugins.commandAttachmentError"));
        return;
      }
      void executePluginCommand(command);
      return;
    }
    void sendPrompt();
  };

  const handleSubmit = (event: FormEvent): void => {
    event.preventDefault();
    submitComposer();
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
    resetPrompt(nextPrompt, scope.sessionId);
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
    resetPrompt(nextPrompt, source.id);
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
    resetPrompt(nextPrompt, conversation.id);
    setSlashMenuOpen(false);
    skillDetailRequest.current += 1;
    setSkillDetailTarget(undefined);
    setSkillDetail(undefined);
    setSkillDetailBusy(false);
    setSkillDetailError(undefined);
    setCompactionSummaryDetail(undefined);
    setPluginCommandDetail(undefined);
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

  const selectSlashMenuItem = (item: SlashMenuItem): void => {
    if (item.disabled) return;
    if (item.builtin === "compact") {
      void runCompactionCommand();
      return;
    }
    if (item.builtin === "fork") {
      void runForkCommand();
      return;
    }
    if (item.builtin === "btw") {
      openSideChatCommand();
      return;
    }
    if (item.builtin === "mcp") {
      void openMcpStatus();
      return;
    }
    if (!item.plugin || !activeConversation) return;
    const value = `/${pluginCommandLabel(item.plugin)} `;
    resetPrompt(value, activeConversation.id);
    setSlashMenuOpen(false);
    window.requestAnimationFrame(() => {
      const textarea = textareaRef.current;
      if (!textarea) return;
      textarea.focus();
      textarea.setSelectionRange(value.length, value.length);
    });
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
      if (slashMenuItems.length === 0) return;
      setSlashMenuActiveIndex((current) => {
        const delta = event.key === "ArrowDown" ? 1 : -1;
        return (
          (current + delta + slashMenuItems.length) %
          slashMenuItems.length
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
      const item = slashMenuItems[slashMenuActiveIndex];
      if (item) selectSlashMenuItem(item);
      return;
    }
    if (
      event.key.toLowerCase() === "z" &&
      (event.ctrlKey || event.metaKey) &&
      !event.altKey &&
      !event.shiftKey
    ) {
      event.preventDefault();
      const conversationId = activeConversation?.id;
      const history = conversationId
        ? promptUndoHistoriesRef.current[conversationId]
        : undefined;
      if (history && canUndoPromptEdit(history)) {
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
      submitComposer();
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
        <AppSidebar
          desktop={desktop}
          activeProject={activeProject}
          activeConversation={activeConversation}
          inFlightTurns={inFlightTurns}
          interactions={interactions}
          unreadCompletedConversations={unreadCompletedConversations}
          auth={auth}
          accountProfile={accountProfile}
          accountUsage={accountUsage}
          accountUsageBusy={accountUsageBusy}
          accountUsageError={accountUsageError}
          profileOpen={profileOpen}
          sidebarCollapsed={sidebarCollapsed}
          mobileLayout={mobileLayout}
          mobileSidebarOpen={mobileSidebarOpen}
          profileRef={profileRef}
          onToggleSidebar={toggleSidebar}
          onAddProject={() => void addProject()}
          onOpenSidebar={openSidebar}
          onToggleProject={toggleProject}
          onCreateConversation={(project, event) =>
            void createConversation(project, event)
          }
          onSelectConversation={selectConversation}
          onSetRemovalTarget={setRemovalTarget}
          onToggleProfile={toggleProfile}
          onRefreshAccountUsage={() => void loadAccountUsage()}
          onLogin={() => void startLogin()}
          onOpenSettings={openSettings}
          onSignOut={() => void signOut()}
          onCloseMobileNavigation={closeMobileNavigation}
        />


        <main
          className="workspace"
          inert={mobileLayout && mobileSidebarOpen}
        >
        {activeProject && activeConversation ? (
          <>
            <header className="chat-header">
              <button
                className="mobile-workspace-trigger"
                ref={mobileMenuButtonRef}
                type="button"
                title={t("sidebar.openWorkspace")}
                aria-label={t("sidebar.openWorkspace")}
                aria-expanded={mobileSidebarOpen}
                onClick={openSidebar}
              >
                <AccountAvatar profile={accountProfile} size={28} />
              </button>
              <div className="chat-heading">
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
                <button
                  className="icon-button cron-manager-trigger"
                  type="button"
                  title={t("cron.open")}
                  aria-label={t("cron.open")}
                  disabled={!activeAgentScope}
                  onClick={() => setCronManagerOpen(true)}
                >
                  <CalendarClock size={17} />
                  {activeCronTaskCount > 0 && (
                    <span className="cron-manager-badge" aria-hidden="true">
                      {cronTaskBadge(activeCronTaskCount)}
                    </span>
                  )}
                </button>
                <button
                  className="icon-button"
                  type="button"
                  title={t("agents.open")}
                  aria-label={t("agents.open")}
                  onClick={() => setAgentManagerOpen(true)}
                >
                  <Bot size={17} />
                </button>
                <button className="icon-button" type="button" title={t("conversation.create")} onClick={() => void createConversation(activeProject)}>
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
                      subagentRuns={historicalSubagentRuns}
                      subagentHistories={activeSubagentHistories}
                      onLoadSubagentHistory={(agentId, force) =>
                        loadSubagentHistory(activeConversation.id, agentId, force)
                      }
                      messageDurations={
                        messageDurations[activeConversation.id] ?? {}
                      }
                      undoableUserMessageId={undoableUserMessageId}
                      onUndoUserMessage={setUndoMessageTarget}
                      copiedMessageId={copiedMessage}
                      onCopy={copyMessage}
                      onSkillOpen={(name) =>
                        void openSkillDetail({ name })
                      }
                      onPluginCommandOpen={openPluginCommandDetail}
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
                  {pendingHandoffTurns.map((turn) => (
                    <LiveTurnView
                      key={`handoff-${turn.turnId ?? turn.createdAt}`}
                      turn={turn}
                      outlineId={`handoff-${turn.turnId ?? turn.createdAt}`}
                      subagentRuns={activeSubagentRuns}
                      subagentLiveTurns={activeSubagentLiveTurns}
                      onSkillOpen={(name) => void openSkillDetail({ name })}
                      onPluginCommandOpen={openPluginCommandDetail}
                      onCompactionSummaryOpen={openCompactionSummary}
                    />
                  ))}
                  {activeTurn && (
                    <LiveTurnView
                      turn={activeTurn}
                      liveCompaction={activeCompaction}
                      compactionSummary={activeLiveCompactionSummary}
                      outlineId={liveOutlineTurnId}
                      subagentRuns={activeSubagentRuns}
                      subagentLiveTurns={activeSubagentLiveTurns}
                      onSkillOpen={(name) =>
                        void openSkillDetail({ name })
                      }
                      onPluginCommandOpen={openPluginCommandDetail}
                      onCompactionSummaryOpen={openCompactionSummary}
                    />
                  )}
                  {!activeTurn &&
                    activeCompaction &&
                    (activeCompaction.phase !== "completed" ||
                      !compactionHistoryReady[activeConversation.id]) && (
                    <CompactionNotice event={activeCompaction} />
                  )}
                </div>
              )}
            </div>

            <ComposerDock
              queuedMessages={
                activeQueuedPrompts.length > 0 ||
                activeRemoteQueuedPrompts.length > 0 ? (
                  <QueuedPromptList
                    prompts={activeQueuedPrompts}
                    remotePrompts={activeRemoteQueuedPrompts}
                    canSteer={isStreaming}
                    onRemove={removeQueuedPrompt}
                    onSteer={(queuedPromptId) =>
                      void steerQueuedPrompt(queuedPromptId)
                    }
                    onSkillOpen={(name) =>
                      void openSkillDetail({ name })
                    }
                  />
                ) : undefined
              }
              activeAgentScope={activeAgentScope}
              activeAgentUsage={activeAgentUsage}
              activeApproval={activeApproval}
              activeBackgroundTasks={activeBackgroundTasks}
              activeCompaction={activeCompaction}
              activeContextUsage={activeContextUsage}
              activeGoal={activeGoal}
              activeGoalMode={activeGoalMode}
              activePlan={activePlan}
              activeQuestion={activeQuestion}
              activeSwarmMode={activeSwarmMode}
              activeTodos={activeTodos}
              attachmentInputRef={attachmentInputRef}
              availableSkills={availableSkills}
              composerAddOpen={composerAddOpen}
              composerAddRef={composerAddRef}
              composerHasContent={composerHasContent}
              effort={effort}
              forkCommandBusy={forkCommandBusy}
              hasBlockingInteraction={hasBlockingInteraction}
              isHistoryLoading={isHistoryLoading}
              isStreaming={isStreaming}
              modeBusy={modeBusy}
              mcpStatusBusy={mcpStatusBusy}
              mcpStatusError={mcpStatusError}
              mcpStatusOpen={mcpStatusOpen}
              mcpServers={mcpServers}
              modelBusy={modelBusy}
              models={models}
              modelsBusy={modelsBusy}
              permissionMode={permissionMode}
              prompt={prompt}
              promptAttachments={promptAttachments}
              promptCompositionRef={promptCompositionRef}
              promptSkills={promptSkills}
              resolvingInteraction={resolvingInteraction}
              selectedModel={selectedModel}
              showStopButton={showStopButton}
              skillsBusy={skillsBusy}
              skillsError={skillsError}
              slashMenuActiveIndex={slashMenuActiveIndex}
              slashMenuItems={slashMenuItems}
              slashMenuOpen={slashMenuOpen}
              supportedThinkingLevels={supportedThinkingLevels}
              textareaRef={textareaRef}
              cancelActiveTurn={cancelActiveTurn}
              chooseEffort={chooseEffort}
              chooseModel={chooseModel}
              choosePermissionMode={choosePermissionMode}
              controlActiveGoal={controlActiveGoal}
              handleAttachmentInput={handleAttachmentInput}
              handlePromptKeyDown={handlePromptKeyDown}
              handlePromptPaste={handlePromptPaste}
              handleSubmit={handleSubmit}
              loadAvailableSkills={loadAvailableSkills}
              loadBackgroundTaskOutput={loadBackgroundTaskOutput}
              stopBackgroundTask={stopBackgroundTask}
              openSkillDetail={openSkillDetail}
              closeMcpStatus={closeMcpStatus}
              resolveApproval={resolveApproval}
              respondToInteraction={respondToInteraction}
              selectSlashMenuItem={selectSlashMenuItem}
              selectPromptSkill={selectPromptSkill}
              setComposerAddOpen={setComposerAddOpen}
              setGoalEditTarget={setGoalEditTarget}
              setPromptAttachments={setPromptAttachments}
              setPromptSkills={setPromptSkills}
              setSlashMenuActiveIndex={setSlashMenuActiveIndex}
              setSlashMenuOpen={setSlashMenuOpen}
              syncSlashMenu={syncSlashMenu}
              toggleComposerAdd={toggleComposerAdd}
              toggleGoalMode={toggleGoalMode}
              togglePlanMode={togglePlanMode}
              toggleSwarmMode={toggleSwarmMode}
              updatePrompt={updatePrompt}
            />
          </>
        ) : activeProject ? (
          <ProjectConversationEmpty
            collapsed={sidebarCollapsed}
            menuButtonRef={mobileMenuButtonRef}
            onExpand={openSidebar}
            onCreateConversation={() => void createConversation(activeProject)}
          />
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
        ) : pluginCommandDetail ? (
          <PluginCommandDetailSidebar
            command={pluginCommandDetail}
            onClose={() => setPluginCommandDetail(undefined)}
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

      <AppOverlays
        loginOpen={loginOpen}
        loginBusy={loginBusy}
        deviceCode={deviceCode}
        webAuthOpen={webAuthOpen}
        removalTarget={removalTarget}
        removalBusy={removalBusy}
        undoMessageTarget={undoMessageTarget}
        undoMessageBusy={undoMessageBusy}
        goalEditTarget={goalEditTarget}
        goalEditBusy={goalEditBusy}
        directoryPickerOpen={directoryPickerOpen}
        settingsOpen={settingsOpen}
        agentManagerOpen={agentManagerOpen}
        agentManagerWorkspace={activeProject ? { id: activeProject.id, name: activeProject.name } : undefined}
        cronManagerOpen={cronManagerOpen}
        cronManagerSession={activeConversation ? { id: activeConversation.id } : undefined}
        appVersion={appVersion}
        auth={auth}
        accountProfile={accountProfile}
        accountUsage={accountUsage}
        accountUsageBusy={accountUsageBusy}
        accountUsageError={accountUsageError}
        colorScheme={colorScheme}
        fontSize={fontSize}
        customColors={customColors[colorScheme]}
        customFonts={customFonts}
        language={language}
        notificationsEnabled={notificationsEnabled}
        autoConversationTitlesEnabled={autoConversationTitlesEnabled}
        conversationTitleModel={conversationTitleModel}
        models={models}
        notice={notice}
        onCloseLogin={() => {
          if (!loginBusy) setLoginOpen(false);
        }}
        onStartLogin={() => void startLogin()}
        onRefreshAccountUsage={() => void loadAccountUsage()}
        onSignOut={() => void signOut()}
        onSubmitCredential={(credential) => {
          setWebCredential(credential);
          setWebAuthOpen(false);
          window.location.reload();
        }}
        onCloseRemoval={() => {
          if (!removalBusy) setRemovalTarget(undefined);
        }}
        onConfirmRemoval={() => void confirmRemoval()}
        onCloseUndoMessage={() => {
          if (!undoMessageBusy) setUndoMessageTarget(undefined);
        }}
        onConfirmUndoMessage={() => void confirmUndoMessage()}
        onCloseGoalEdit={() => {
          if (!goalEditBusy) setGoalEditTarget(undefined);
        }}
        onConfirmGoalEdit={(goal, objective) =>
          void editActiveGoal(goal, objective)
        }
        onCloseDirectoryPicker={() => setDirectoryPickerOpen(false)}
        onSelectDirectory={(path) => {
          setDirectoryPickerOpen(false);
          void addProjectPath(path);
        }}
        onColorSchemeChange={updateColorScheme}
        onFontSizeChange={updateFontSize}
        onCustomColorChange={updateCustomColor}
        onCustomFontsChange={updateCustomFonts}
        onCustomFontNameChange={updateCustomFontName}
        onLanguageChange={updateLanguage}
        onNotificationsEnabledChange={updateNotificationsEnabled}
        onAutoConversationTitlesEnabledChange={
          updateAutoConversationTitlesEnabled
        }
        onConversationTitleModelChange={updateConversationTitleModel}
        onProvidersChanged={() => void loadModels()}
        onPluginsChanged={() => setPluginCommandRevision((value) => value + 1)}
        onCloseSettings={closeSettings}
        onCloseAgentManager={() => setAgentManagerOpen(false)}
        onCloseCronManager={() => setCronManagerOpen(false)}
        onCronTaskCountChange={updateCronTaskCount}
        onDismissNotice={() => setNotice(undefined)}
      />


    </div>
  );
}
