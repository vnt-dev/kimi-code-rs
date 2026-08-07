import type { SessionSummary } from "./types";

export type ArchivedSessionSort =
  | "archived-desc"
  | "created-desc"
  | "name-asc";

export interface ArchivedSessionGroup {
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
  const groups = new Map<string, SessionSummary[]>();
  for (const session of sessions) {
    const path = archivedSessionPath(session, unknownWorkspaceLabel);
    const group = groups.get(path);
    if (group) {
      group.push(session);
    } else {
      groups.set(path, [session]);
    }
  }
  return [...groups].map(([path, groupedSessions]) => ({
    path,
    sessions: groupedSessions,
  }));
}

export function formatArchivedTime(timestamp: number): string {
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return "—";
  const pad = (value: number): string => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
