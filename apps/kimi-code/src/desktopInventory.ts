import type { DesktopState } from "./types.ts";

export function mergeDesktopInventory(
  current: DesktopState,
  incoming: DesktopState,
): DesktopState {
  const projects = incoming.projects.map((project) => {
    const existing = current.projects.find(
      (candidate) => candidate.id === project.id || candidate.path === project.path,
    );
    if (!existing) return project;
    return {
      ...project,
      accent: existing.accent,
      expanded: existing.expanded,
      conversations: project.conversations.map((conversation) => {
        const previous = existing.conversations.find(
          (candidate) => candidate.id === conversation.id,
        );
        return previous
          ? {
              ...conversation,
              modelId: previous.modelId,
              thinkingLevel: previous.thinkingLevel,
              permissionMode: previous.permissionMode,
            }
          : conversation;
      }),
    };
  });
  const activeProjectId = projects.some(
    (project) => project.id === current.activeProjectId,
  )
    ? current.activeProjectId
    : incoming.activeProjectId;
  const activeProject = projects.find((project) => project.id === activeProjectId);
  const activeConversationId = activeProject?.conversations.some(
    (conversation) => conversation.id === current.activeConversationId,
  )
    ? current.activeConversationId
    : activeProject?.conversations[0]?.id;
  return { projects, activeProjectId, activeConversationId };
}
