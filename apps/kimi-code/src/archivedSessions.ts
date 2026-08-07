import type { SessionSummary } from "./types";

export type ArchivedSessionSort =
  | "archived-desc"
  | "created-desc"
  | "name-asc";

export interface ArchivedSessionGroup {
  workspaceId: string;
  path: string;
  sessions: SessionSummary[];
}

export function archivedSessionTitle(
  session: SessionSummary,
  fallback: string,
): string {
  return session.title?.trim() || session.lastPrompt?.trim() || fallback;
}

export function archivedSessionPath(
  session: SessionSummary,
  fallback: string,
): string {
  return session.cwd?.trim() || fallback;
}

export function filterArchivedSessions(
  sessions: readonly SessionSummary[],
  options: {
    query: string;
    workspacePath: string;
    sort: ArchivedSessionSort;
    untitledLabel: string;
    unknownWorkspaceLabel: string;
    locale: string;
  },
): SessionSummary[] {
  const query = options.query.trim().toLocaleLowerCase(options.locale);
  const filtered = sessions.filter((session) => {
    if (!session.archived) return false;
    const path = archivedSessionPath(session, options.unknownWorkspaceLabel);
    if (options.workspacePath !== "all" && path !== options.workspacePath) {
      return false;
    }
    return (
      query.length === 0 ||
      archivedSessionTitle(session, options.untitledLabel)
        .toLocaleLowerCase(options.locale)
        .includes(query)
    );
  });

  return filtered.sort((left, right) => {
    if (options.sort === "created-desc") {
      return right.createdAt - left.createdAt;
    }
    if (options.sort === "name-asc") {
      return archivedSessionTitle(left, options.untitledLabel).localeCompare(
        archivedSessionTitle(right, options.untitledLabel),
        options.locale,
      );
    }
    return right.updatedAt - left.updatedAt;
  });
}

export function groupArchivedSessions(
  sessions: readonly SessionSummary[],
  unknownWorkspaceLabel: string,
): ArchivedSessionGroup[] {
  const groups = new Map<string, ArchivedSessionGroup>();
  for (const session of sessions) {
    const path = archivedSessionPath(session, unknownWorkspaceLabel);
    const group = groups.get(session.workspaceId);
    if (group) {
      group.sessions.push(session);
    } else {
      groups.set(session.workspaceId, {
        workspaceId: session.workspaceId,
        path,
        sessions: [session],
      });
    }
  }
  return [...groups.values()];
}

export function archivedSessionIdsForWorkspace(
  sessions: readonly SessionSummary[],
  workspaceId: string,
): string[] {
  return sessions
    .filter(
      (session) => session.archived && session.workspaceId === workspaceId,
    )
    .map((session) => session.id);
}

export function removeArchivedSessions(
  sessions: readonly SessionSummary[],
  deletedSessionIds: readonly string[],
): SessionSummary[] {
  const deleted = new Set(deletedSessionIds);
  if (deleted.size === 0) return [...sessions];
  return sessions.filter((session) => !deleted.has(session.id));
}

export function formatArchivedTime(timestamp: number): string {
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return "—";
  const pad = (value: number): string => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
