import type { Conversation, DesktopState, Project } from "./types";

const STORAGE_KEY = "kimi-code.desktop.workspace.v1";
const ACCENTS = ["#8b7cf6", "#5aa9ff", "#47c7a2", "#f0a45d", "#df719d"];

export function createId(prefix: string): string {
  return `${prefix}_${crypto.randomUUID()}`;
}

export function loadState(): DesktopState {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { projects: [] };
    const parsed = JSON.parse(raw) as DesktopState;
    return Array.isArray(parsed.projects) ? parsed : { projects: [] };
  } catch {
    return { projects: [] };
  }
}

export function persistState(state: DesktopState): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
}

export function newConversation(): Conversation {
  const now = Date.now();
  return {
    id: createId("chat"),
    title: "新对话",
    createdAt: now,
    updatedAt: now,
    messages: [],
  };
}

export function newProject(path: string, index: number): Project {
  const normalized = path.replace(/[\\/]+$/, "");
  const name = normalized.split(/[\\/]/).filter(Boolean).at(-1) || "未命名项目";
  const conversation = newConversation();
  return {
    id: createId("project"),
    name,
    path: normalized,
    accent: ACCENTS[index % ACCENTS.length],
    expanded: true,
    conversations: [conversation],
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
