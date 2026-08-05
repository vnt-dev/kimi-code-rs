import type { Dispatch, RefObject, SetStateAction } from "react";
import { createAgentClient } from "../agentRpc";
import {
  MAIN_AGENT_ID,
  inFlightTurnFromUserMessage,
  isTurnRunning,
  readPromptSubmittedEvent,
  reduceAgentChatEvent,
  reduceQueuedAgentChatEvents,
  reduceQueuedSubagentChatEvents,
  type GoalModeChangedEvent,
  type InFlightTurn,
  type QueuedAgentChatEvent,
  type RemoteQueuedPrompt,
  type SubagentLiveTurns,
} from "../chat/liveTurns";
import type { RenderMessage } from "../chat/history";
import {
  isAgentChatEvent,
  isTaskLifecycleEventType,
  readAgentTaskInfo,
  readTodoItems,
} from "../chat/eventParsing";
import type { SideChatState } from "../components/sidebars/ChatSidebars";
import { mergeDesktopInventory } from "../desktopInventory";
import { t } from "../i18n";
import { isSameLiveUserMessage } from "../liveUserMessage";
import { loadDesktopState } from "../store";
import {
  isSubagentEvent,
  mergeSessionSubagentEvent,
  type SessionSubagentRuns,
} from "../subagentEvents";
import {
  TRANSPORT_AUTH_REQUIRED,
  TRANSPORT_REPLAY_RESET,
  invoke,
  listen,
  type ReplayResetEvent,
} from "../transport";
import type {
  AgentChatEventEnvelope,
  AgentInteraction,
  AgentInteractionsEvent,
  AgentUsageStatus,
  BackgroundTaskView,
  CompactionEvent,
  ContextUsage,
  DesktopState,
  DeviceCode,
  GoalSnapshot,
  PermissionMode,
  PlanData,
  TodoItem,
} from "../types";
import { conciseError } from "../utils/errors";
import {
  BACKGROUND_TASK_DETAIL_TAIL,
  fetchConversationHistory,
  omitSessionKeys,
  type ConversationHistory,
} from "./appUtils";

type Setter<T> = Dispatch<SetStateAction<T>>;

interface AppEventSubscriptions {
  agentChatEventFrame: RefObject<number | undefined>;
  desktopInventoryRequest: RefObject<number>;
  historyRequests: RefObject<Record<string, number>>;
  inFlightTurnsRef: RefObject<Record<string, InFlightTurn>>;
  queuedAgentChatEvents: RefObject<QueuedAgentChatEvent[]>;
  sideChatAgentId: RefObject<string | undefined>;
  sideChatAgentIds: RefObject<Set<string>>;
  loadBackgroundTaskOutput: (
    scope: { sessionId: string; agentId: string },
    taskId: string,
    tail?: number,
  ) => Promise<void>;
  refreshBackgroundTasks: (
    scope: { sessionId: string; agentId: string },
  ) => Promise<void>;
  setAgentUsages: Setter<Record<string, AgentUsageStatus>>;
  setBackgroundTasks: Setter<Record<string, BackgroundTaskView[]>>;
  setCompactionHistoryReady: Setter<Record<string, boolean>>;
  setCompactions: Setter<Record<string, CompactionEvent>>;
  setContextUsages: Setter<Record<string, ContextUsage>>;
  setDesktop: Setter<DesktopState>;
  setDeviceCode: Setter<DeviceCode | undefined>;
  setGoalModeBySession: Setter<Record<string, boolean>>;
  setGoals: Setter<Record<string, GoalSnapshot | null>>;
  setHistoryByConversation: Setter<Record<string, ConversationHistory>>;
  setInFlightTurns: Setter<Record<string, InFlightTurn>>;
  setInteractions: Setter<Record<string, AgentInteraction[]>>;
  setLoginOpen: Setter<boolean>;
  setPlans: Setter<Record<string, PlanData | null>>;
  setRemoteQueuedPrompts: Setter<Record<string, RemoteQueuedPrompt[]>>;
  setSessionTodos: Setter<Record<string, TodoItem[]>>;
  setSideChat: Setter<SideChatState | undefined>;
  setSubagentLiveTurns: Setter<SubagentLiveTurns>;
  setSubagentRuns: Setter<SessionSubagentRuns>;
  setSwarmModeBySession: Setter<Record<string, boolean>>;
  setUndoMessageTarget: Setter<RenderMessage | undefined>;
  setWebAuthOpen: Setter<boolean>;
  showNotice: (message: string) => void;
  updateDesktop: (recipe: (current: DesktopState) => DesktopState) => void;
}

export function subscribeToAppEvents({
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
}: AppEventSubscriptions): () => void {
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
                pluginCommand: projected.pluginCommand,
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
}
