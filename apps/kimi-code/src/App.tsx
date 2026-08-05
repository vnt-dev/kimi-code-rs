import {
  Folder,
  Menu,
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
  forkSession,
  getSharedGoalMode,
  listWorkspaceSessions,
  prepareSession,
  subscribeAgentEvents,
  unsubscribeAgentEvents,
  type PluginCommandDef,
} from "./agentRpc";
import { subscribeToAppEvents } from "./app/appEventSubscriptions";
import {
  BACKGROUND_TASK_LIST_LIMIT,
  BACKGROUND_TASK_OUTPUT_TAIL,
  LIVE_TURN_HANDOFF_MS,
  fetchConversationHistory,
  newQueuedPromptId,
  type AgentSubscription,
  type ConversationHistory,
  type PendingAgentSubscription
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
  applyColorScheme,
  loadColorScheme,
  saveColorScheme,
  type ColorScheme,
} from "./appearance";
import {
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
  RemoteQueuedPromptList,
  Welcome,
} from "./components/chat/ConversationViews";
import {
  ChatHeaderTitle,
  CompactionNotice,
  WindowTitleBar
} from "./components/ChatHeader";
import { ComposerDock } from "./components/ComposerDock";
import { ProjectLanding } from "./components/ProjectLanding";
import {
  CompactionSummarySidebar,
  SideChatSidebar,
  SkillDetailSidebar,
  type CompactionSummaryDetail,
  type SideChatState,
  type SkillDetailTarget,
} from "./components/sidebars/ChatSidebars";
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
  type SessionSubagentRuns
} from "./subagentEvents";
import {
  getAppVersion,
  invoke,
  setWebCredential,
  webCredentialRequired
} from "./transport";
import type {
  AccountUsage,
  AgentInteraction,
  AgentUsageStatus,
  AuthStatus,
  BackgroundTaskView,
  CompactionEvent,
  ContextUsage,
  DesktopState,
  DeviceCode,
  GoalSnapshot,
  Model,
  PlanData,
  ProtocolMessage,
  SkillContent,
  SkillDescriptor,
  TodoItem,
  TurnFileChange
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
  const [sideChat, setSideChat] = useState<SideChatState>();
  const [composerAddOpen, setComposerAddOpen] = useState(false);
  const [slashMenuOpen, setSlashMenuOpen] = useState(false);
  const [slashMenuActiveIndex, setSlashMenuActiveIndex] = useState(0);
  const [pluginCommands, setPluginCommands] = useState<PluginCommandDef[]>([]);
  const [pluginCommandRevision, setPluginCommandRevision] = useState(0);
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
  const [inFlightTurns, setInFlightTurns] = useState<
    Record<string, InFlightTurn>
  >({});
  const inFlightTurnsRef = useRef(inFlightTurns);
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

  useEffect(
    () =>
      subscribeToAppEvents({
        agentChatEventFrame,
        desktopInventoryRequest,
        historyRequests,
        inFlightTurnsRef,
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
        setRemoteQueuedPrompts,
        setSessionTodos,
        setSideChat,
        setSubagentLiveTurns,
        setSubagentRuns,
        setSwarmModeBySession,
        setUndoMessageTarget,
        setWebAuthOpen,
        showNotice,
        updateDesktop,
      }),
    [],
  );

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
    if (open) setComposerAddOpen(false);
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
    promptAttachments,
    promptSkills,
    refreshModels,
    resetPrompt,
    selectedModel,
    setAccountUsage,
    setAccountUsageBusy,
    setAccountUsageError,
    setAuth,
    setAvailableSkills,
    setCompactionHistoryReady,
    setCompactionSummaryDetail,
    setComposerAddOpen,
    setDeviceCode,
    setHistoryByConversation,
    setLoginBusy,
    setLoginOpen,
    setMessageDurations,
    setMessageFileChanges,
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
      resetPrompt("", conversationId);
      setPromptAttachments([], conversationId);
      setPromptSkills([], conversationId);
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

  const handleSubmit = (event: FormEvent): void => {
    event.preventDefault();
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
        <AppSidebar
          desktop={desktop}
          activeProject={activeProject}
          activeConversation={activeConversation}
          inFlightTurns={inFlightTurns}
          auth={auth}
          appVersion={appVersion}
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

            <ComposerDock
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
              openSkillDetail={openSkillDetail}
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
        appVersion={appVersion}
        colorScheme={colorScheme}
        language={language}
        notice={notice}
        onCloseLogin={() => {
          if (!loginBusy) setLoginOpen(false);
        }}
        onStartLogin={() => void startLogin()}
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
        onLanguageChange={updateLanguage}
        onPluginsChanged={() => setPluginCommandRevision((value) => value + 1)}
        onCloseSettings={closeSettings}
        onDismissNotice={() => setNotice(undefined)}
      />


    </div>
  );
}
