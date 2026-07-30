import { invoke } from "@tauri-apps/api/core";

import type {
  AgentConfig,
  AgentPromptPart,
  AgentTaskInfo,
  AgentUsageStatus,
  PermissionMode,
  PlanData,
  PreparedSession,
  SessionSummary,
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
    prompt(input: string | readonly AgentPromptPart[]) {
      return callAgentRpc<{ turnId: number } | null>(scope, "prompt", {
        input:
          typeof input === "string"
            ? [{ type: "text", text: input }]
            : [...input],
      });
    },
    cancel(turnId?: number) {
      return callAgentRpc<void>(scope, "cancel", { turnId });
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
    getTodos() {
      return callAgentRpc<TodoItem[]>(scope, "getTodos");
    },
    getUsage() {
      return callAgentRpc<AgentUsageStatus>(scope, "getUsage");
    },
    getTasks(options: { activeOnly?: boolean; limit?: number } = {}) {
      return callAgentRpc<AgentTaskInfo[]>(scope, "getTasks", options);
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

export function removeWorkspace(workspaceId: string): Promise<void> {
  return invoke<void>("remove_workspace", { workspaceId });
}

export function listWorkspaceSessions(
  workspaceId: string,
): Promise<SessionSummary[]> {
  return invoke<SessionSummary[]>("list_workspace_sessions", { workspaceId });
}

export function archiveSession(sessionId: string): Promise<void> {
  return invoke<void>("archive_session", { sessionId });
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
  return invoke<string>("subscribe_agent_events", {
    sessionId: scope.sessionId,
    agentId: scope.agentId,
  });
}

export function unsubscribeAgentEvents(subscriptionId: string): Promise<void> {
  return invoke<void>("unsubscribe_agent_events", { subscriptionId });
}
