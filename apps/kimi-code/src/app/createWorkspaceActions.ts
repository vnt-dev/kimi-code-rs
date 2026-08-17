import type {
  Dispatch,
  MouseEvent,
  RefObject,
  SetStateAction,
} from "react";
import {
  archiveSession,
  createAgentClient,
  createOrTouchWorkspace,
  listWorkspaceSessions,
  prepareSession,
  removeWorkspace,
  setDefaultModel,
  setSharedGoalMode,
} from "../agentRpc";
import type {
  InFlightTurn,
  PromptAttachment,
  QueuedPrompt,
  RemoteQueuedPrompt,
  SubagentLiveTurns,
} from "../chat/liveTurns";
import type { RemovalTarget } from "../components/AppDialogs";
import { t } from "../i18n";
import {
  normalizeThinkingLevel,
  thinkingLevelsForModel,
} from "../modelControls";
import type { PromptDrafts } from "../promptDrafts";
import { removePromptDrafts } from "../promptDrafts";
import type { PromptUndoHistory } from "../promptUndo";
import {
  conversationFromSession,
  projectFromWorkspace,
} from "../store";
import type { SessionSubagentRuns } from "../subagentEvents";
import {
  isDesktop,
  pickNativeDirectory,
} from "../transport";
import type {
  AgentInteraction,
  AgentUsageStatus,
  BackgroundTaskView,
  ContextUsage,
  Conversation,
  DesktopState,
  GoalSnapshot,
  Model,
  PermissionMode,
  PlanData,
  Project,
  SkillDescriptor,
  TodoItem
} from "../types";
import type { LiveCompactionEvent } from "../chat/conversationTimeline";
import { conciseError } from "../utils/errors";
import {
  omitSessionKeys,
  type ConversationHistory,
} from "./appUtils";

type Setter<T> = Dispatch<SetStateAction<T>>;

interface WorkspaceActionOptions {
  activeAgentScope?: { sessionId: string; agentId: string };
  activeConversation?: Conversation;
  activeGoal?: GoalSnapshot | null;
  activeGoalMode: boolean;
  activePlan?: PlanData | null;
  activeProject?: Project;
  activeSwarmMode: boolean;
  backgroundTaskRequests: RefObject<Record<string, number>>;
  closeMobileNavigation: () => void;
  desktop: DesktopState;
  effort: string;
  expandDesktopSidebar: () => void;
  historyRequests: RefObject<Record<string, number>>;
  inFlightTurnsRef: RefObject<Record<string, InFlightTurn>>;
  isStreaming: boolean;
  mobileLayout: boolean;
  modeBusy: boolean;
  modelBusy: boolean;
  models: Model[];
  permissionMode: PermissionMode;
  promptUndoHistoriesRef: RefObject<Record<string, PromptUndoHistory>>;
  refreshAgentState: (
    scope: { sessionId: string; agentId: string },
  ) => Promise<void>;
  releaseAgentSubscription: (sessionId: string) => void;
  removalBusy: boolean;
  removalTarget?: RemovalTarget;
  selectedModel?: Model;
  setAgentUsages: Setter<Record<string, AgentUsageStatus>>;
  setBackgroundTasks: Setter<Record<string, BackgroundTaskView[]>>;
  setCompactionHistoryReady: Setter<Record<string, boolean>>;
  setCompactions: Setter<Record<string, LiveCompactionEvent>>;
  setContextUsages: Setter<Record<string, ContextUsage>>;
  setDirectoryPickerOpen: Setter<boolean>;
  setGoalEditBusy: Setter<boolean>;
  setGoalEditTarget: Setter<GoalSnapshot | undefined>;
  setGoalModeBySession: Setter<Record<string, boolean>>;
  setGoals: Setter<Record<string, GoalSnapshot | null>>;
  setHistoryByConversation: Setter<Record<string, ConversationHistory>>;
  setInFlightTurns: Setter<Record<string, InFlightTurn>>;
  setInteractions: Setter<Record<string, AgentInteraction[]>>;
  setMessageDurations: Setter<Record<string, Record<string, number>>>;
  setModeBusy: Setter<boolean>;
  setModelBusy: Setter<boolean>;
  setModels: Setter<Model[]>;
  setPlans: Setter<Record<string, PlanData | null>>;
  setPromptDrafts: Setter<
    PromptDrafts<PromptAttachment, SkillDescriptor>
  >;
  setQueuedPrompts: Setter<Record<string, QueuedPrompt[]>>;
  setRemoteQueuedPrompts: Setter<Record<string, RemoteQueuedPrompt[]>>;
  setRemovalBusy: Setter<boolean>;
  setRemovalTarget: Setter<RemovalTarget | undefined>;
  setResolvingInteraction: Setter<string | undefined>;
  setSessionTodos: Setter<Record<string, TodoItem[]>>;
  setSubagentLiveTurns: Setter<SubagentLiveTurns>;
  setSubagentRuns: Setter<SessionSubagentRuns>;
  setSwarmModeBySession: Setter<Record<string, boolean>>;
  showNotice: (message: string) => void;
  updateDesktop: (recipe: (current: DesktopState) => DesktopState) => void;
}

export function createWorkspaceActions({
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
}: WorkspaceActionOptions) {
  const forgetSessionState = (sessionIds: string[]): void => {
    const ids = new Set(sessionIds);
    if (ids.size === 0) return;
    for (const sessionId of ids) {
      delete historyRequests.current[sessionId];
      delete backgroundTaskRequests.current[sessionId];
      delete promptUndoHistoriesRef.current[sessionId];
      releaseAgentSubscription(sessionId);
    }
    setPromptDrafts((current) => removePromptDrafts(current, ids));
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
          activeConversationId: project.conversations[0]?.id,
        };
      });
      if (mobileLayout) closeMobileNavigation();
      else expandDesktopSidebar();
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

  return {
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
  };
}
