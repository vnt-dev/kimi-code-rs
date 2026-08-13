import {
  invoke,
  subscribeAgentEventsTransport,
  unsubscribeAgentEventsTransport,
} from "./transport";

import type {
  AgentConfig,
  AgentPromptPart,
  AgentTaskInfo,
  AgentUsageStatus,
  GoalSnapshot,
  GoalToolResult,
  PermissionMode,
  PlanData,
  PreparedSession,
  SessionSummary,
  SkillContent,
  SkillDescriptor,
  TodoItem,
  Workspace,
} from "./types";

export interface AgentScope {
  sessionId: string;
  agentId: string;
}

export type AgentRpcMethod =
  | "prompt"
  | "runShellCommand"
  | "cancelShellCommand"
  | "steer"
  | "cancel"
  | "undoHistory"
  | "setThinking"
  | "setPermission"
  | "renameSession"
  | "generateConversationTitle"
  | "setModel"
  | "getModel"
  | "enterPlan"
  | "cancelPlan"
  | "clearPlan"
  | "enterSwarm"
  | "exitSwarm"
  | "getSwarmMode"
  | "startBtw"
  | "beginCompaction"
  | "cancelCompaction"
  | "registerTool"
  | "unregisterTool"
  | "setActiveTools"
  | "stopTask"
  | "detachTask"
  | "clearContext"
  | "activateSkill"
  | "listPluginCommands"
  | "listMcpServers"
  | "activatePluginCommand"
  | "createGoal"
  | "getGoal"
  | "pauseGoal"
  | "resumeGoal"
  | "cancelGoal"
  | "getTaskOutput"
  | "getContext"
  | "getConfig"
  | "getPermission"
  | "getPlan"
  | "getTodos"
  | "getUsage"
  | "getTools"
  | "getTasks";

type RpcPayload = Record<string, unknown>;

export type AgentPromptSubmitStatus =
  | "queued"
  | "running"
  | "steered"
  | "completed"
  | "failed"
  | "cancelled"
  | "blocked";

export interface AgentPromptSubmitResult {
  promptId: string;
  turnId?: number;
  status: AgentPromptSubmitStatus;
}

export interface AgentPromptSkill {
  name: string;
  args?: string;
}

export interface PluginCommandDef {
  pluginId: string;
  name: string;
  description: string;
  body: string;
  path: string;
}

export type McpServerStatus =
  | "pending"
  | "connected"
  | "failed"
  | "disabled"
  | "needs-auth";

export interface McpServerInfo {
  name: string;
  transport: "stdio" | "http" | "sse";
  status: McpServerStatus;
  toolCount: number;
  error?: string;
}

export interface AgentPromptOptions {
  promptId?: string;
  skills?: readonly AgentPromptSkill[];
}

export async function callAgentRpc<T>(
  scope: AgentScope,
  method: AgentRpcMethod,
  payload: RpcPayload = {},
): Promise<T> {
  return invoke<T>("agent_rpc", {
    request: {
      ...scope,
      method,
      payload,
    },
  });
}

export function createAgentClient(scope: AgentScope) {
  return {
    prompt(
      input: string | readonly AgentPromptPart[],
      options: AgentPromptOptions = {},
    ) {
      return callAgentRpc<AgentPromptSubmitResult>(scope, "prompt", {
        ...(options.promptId ? { promptId: options.promptId } : {}),
        input:
          typeof input === "string"
            ? [{ type: "text", text: input }]
            : [...input],
        ...(options.skills?.length
          ? { skills: options.skills.map((skill) => ({ ...skill })) }
          : {}),
      });
    },
    steer(input: string | readonly AgentPromptPart[], promptId?: string) {
      return callAgentRpc<AgentPromptSubmitResult>(scope, "steer", {
        ...(promptId ? { promptId } : {}),
        input:
          typeof input === "string"
            ? [{ type: "text", text: input }]
            : [...input],
      });
    },
    activateSkill(name: string, args?: string) {
      return callAgentRpc<void>(scope, "activateSkill", {
        name,
        ...(args ? { args } : {}),
      });
    },
    listPluginCommands() {
      return callAgentRpc<PluginCommandDef[]>(scope, "listPluginCommands");
    },
    listMcpServers() {
      return callAgentRpc<McpServerInfo[]>(scope, "listMcpServers");
    },
    activatePluginCommand(pluginId: string, commandName: string, args?: string) {
      return callAgentRpc<void>(scope, "activatePluginCommand", {
        pluginId,
        commandName,
        ...(args ? { args } : {}),
      });
    },
    cancel(turnId?: number) {
      return callAgentRpc<void>(scope, "cancel", { turnId });
    },
    undoHistory(count = 1) {
      return callAgentRpc<number>(scope, "undoHistory", { count });
    },
    setModel(model: string) {
      return callAgentRpc<{ model: string; providerName?: string }>(
        scope,
        "setModel",
        { model },
      );
    },
    getModel() {
      return callAgentRpc<string>(scope, "getModel");
    },
    getConfig() {
      return callAgentRpc<AgentConfig>(scope, "getConfig");
    },
    setThinking(level: string) {
      return callAgentRpc<void>(scope, "setThinking", { level });
    },
    setPermission(mode: PermissionMode) {
      return callAgentRpc<void>(scope, "setPermission", { mode });
    },
    getPermission() {
      return callAgentRpc<{ mode: PermissionMode }>(scope, "getPermission");
    },
    renameSession(title: string) {
      return callAgentRpc<void>(scope, "renameSession", { title });
    },
    generateConversationTitle(text: string, model?: string) {
      return callAgentRpc<string | null>(scope, "generateConversationTitle", {
        text,
        ...(model ? { model } : {}),
      });
    },
    enterPlan() {
      return callAgentRpc<void>(scope, "enterPlan");
    },
    cancelPlan(id?: string) {
      return callAgentRpc<void>(scope, "cancelPlan", { id });
    },
    clearPlan() {
      return callAgentRpc<void>(scope, "clearPlan");
    },
    getPlan() {
      return callAgentRpc<PlanData | null>(scope, "getPlan");
    },
    enterSwarm(trigger: "manual" | "task" = "manual") {
      return callAgentRpc<void>(scope, "enterSwarm", { trigger });
    },
    exitSwarm() {
      return callAgentRpc<void>(scope, "exitSwarm");
    },
    getSwarmMode() {
      return callAgentRpc<boolean>(scope, "getSwarmMode");
    },
    createGoal(
      objective: string,
      replace = false,
      completionCriterion?: string,
    ) {
      return callAgentRpc<GoalSnapshot>(scope, "createGoal", {
        objective,
        replace,
        ...(completionCriterion ? { completionCriterion } : {}),
      });
    },
    getGoal() {
      return callAgentRpc<GoalToolResult>(scope, "getGoal");
    },
    pauseGoal() {
      return callAgentRpc<GoalSnapshot>(scope, "pauseGoal");
    },
    resumeGoal() {
      return callAgentRpc<GoalSnapshot>(scope, "resumeGoal");
    },
    cancelGoal() {
      return callAgentRpc<GoalSnapshot>(scope, "cancelGoal");
    },
    getTodos() {
      return callAgentRpc<TodoItem[]>(scope, "getTodos");
    },
    getUsage() {
      return callAgentRpc<AgentUsageStatus>(scope, "getUsage");
    },
    getTasks(options: { activeOnly?: boolean; limit?: number } = {}) {
      return callAgentRpc<AgentTaskInfo[]>(scope, "getTasks", options);
    },
    stopTask(taskId: string, reason?: string) {
      return callAgentRpc<void>(scope, "stopTask", {
        taskId,
        ...(reason ? { reason } : {}),
      });
    },
    startBtw() {
      return callAgentRpc<string>(scope, "startBtw");
    },
    beginCompaction(instruction?: string) {
      return callAgentRpc<void>(scope, "beginCompaction", {
        ...(instruction ? { instruction } : {}),
      });
    },
    getTaskOutput(taskId: string, tail = 16_384) {
      return callAgentRpc<string>(scope, "getTaskOutput", { taskId, tail });
    },
  };
}

export function listWorkspaces(): Promise<Workspace[]> {
  return invoke<Workspace[]>("list_workspaces");
}

export function createOrTouchWorkspace(root: string): Promise<Workspace> {
  return invoke<Workspace>("create_or_touch_workspace", { root });
}

export function getSharedGoalMode(sessionId: string): Promise<boolean> {
  return invoke<boolean>("get_goal_mode", { sessionId });
}

export function setSharedGoalMode(
  sessionId: string,
  enabled: boolean,
): Promise<void> {
  return invoke<void>("set_goal_mode", { sessionId, enabled });
}

export function removeWorkspace(workspaceId: string): Promise<void> {
  return invoke<void>("remove_workspace", { workspaceId });
}

export function listWorkspaceSessions(
  workspaceId: string,
): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>("list_workspace_sessions", { workspaceId });
}

export function listArchivedSessions(): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>("list_archived_sessions");
}

export function deleteArchivedSessions(
  sessionIds: string[],
): Promise<string[]> {
  return invoke<string[]>("delete_archived_sessions", { sessionIds });
}

export function forkSession(sessionId: string): Promise<string> {
  return invoke<string>("fork_session", { sessionId });
}

export function archiveSession(sessionId: string): Promise<void> {
  return invoke<void>("archive_session", { sessionId });
}

export function restoreSession(sessionId: string): Promise<SessionSummary> {
  return invoke<SessionSummary>("restore_session", { sessionId });
}

export function listSkills(sessionId: string): Promise<SkillDescriptor[]> {
  return invoke<SkillDescriptor[]>("list_skills", { sessionId });
}

export function getSkillContent(
  sessionId: string,
  name: string,
): Promise<SkillContent> {
  return invoke<SkillContent>("get_skill_content", { sessionId, name });
}

export function setDefaultModel(model: string): Promise<void> {
  return invoke<void>("set_default_model", { model });
}

export function prepareSession(input: {
  sessionId?: string;
  workDir: string;
  model?: string;
  thinking?: string;
  permission?: PermissionMode;
}): Promise<PreparedSession> {
  return invoke<PreparedSession>("prepare_session", { request: input });
}

export function subscribeAgentEvents(scope: AgentScope): Promise<string> {
  return subscribeAgentEventsTransport(scope);
}

export function unsubscribeAgentEvents(subscriptionId: string): Promise<void> {
  return unsubscribeAgentEventsTransport(subscriptionId);
}
