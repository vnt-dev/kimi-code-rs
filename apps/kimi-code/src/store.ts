import { listWorkspaces, listWorkspaceSessions } from "./agentRpc";
import type {
  Conversation,
  DesktopState,
  Project,
  SessionSummary,
  Workspace,
} from "./types";

const ACCENTS = ["#8b7cf6", "#5aa9ff", "#47c7a2", "#f0a45d", "#df719d"];

function toConversation(session: SessionSummary): Conversation {
  return {
    id: session.id,
    title: session.title || session.lastPrompt || "新对话",
    createdAt: session.createdAt,
    updatedAt: session.updatedAt,
  };
}

function toProject(
  workspace: Workspace,
  sessions: SessionSummary[],
  index: number,
): Project {
  return {
    id: workspace.id,
    name: workspace.name,
    path: workspace.root,
    accent: ACCENTS[index % ACCENTS.length],
    expanded: true,
    conversations: sessions.map(toConversation),
  };
}

export async function loadDesktopState(): Promise<DesktopState> {
  const workspaces = await listWorkspaces();
  const sessionLists = await Promise.all(
    workspaces.map((workspace) => listWorkspaceSessions(workspace.id)),
  );
  const projects = workspaces.map((workspace, index) =>
    toProject(workspace, sessionLists[index], index),
  );
  const project = projects[0];
  return {
    projects,
    activeProjectId: project?.id,
    activeConversationId: project?.conversations[0]?.id,
  };
}

export function projectFromWorkspace(
  workspace: Workspace,
  index: number,
): Project {
  return toProject(workspace, [], index);
}

export function conversationFromSession(
  sessionId: string,
  now = Date.now(),
): Conversation {
  return {
    id: sessionId,
    title: "新对话",
    createdAt: now,
    updatedAt: now,
  };
}

export function getActive(state: DesktopState): {
  project?: Project;
  conversation?: Conversation;
} {
  const project = state.projects.find((item) => item.id === state.activeProjectId);
  const conversation = project?.conversations.find(
    (item) => item.id === state.activeConversationId,
  );
  return { project, conversation };
}
