import {
  CalendarDays,
  Clock3,
  Folder,
  ListOrdered,
  RefreshCw,
  RotateCcw,
  Search,
  Trash2,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import {
  deleteArchivedSessions,
  listArchivedSessions,
  restoreSession,
} from "./agentRpc";
import {
  archivedSessionIdsForWorkspace,
  archivedSessionPath,
  archivedSessionTitle,
  filterArchivedSessions,
  formatArchivedTime,
  groupArchivedSessions,
  removeArchivedSessions,
  type ArchivedSessionSort,
} from "./archivedSessions";
import SettingsSelect from "./components/SettingsSelect";
import { getLanguage, t } from "./i18n";
import type { SessionSummary } from "./types";
import { conciseError } from "./utils/errors";

const ALL_WORKSPACES = "all";

type DeleteTarget =
  | {
      kind: "session";
      sessionIds: string[];
      label: string;
    }
  | {
      kind: "workspace";
      sessionIds: string[];
      label: string;
    };

export default function ArchivedSessionsSettings() {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [query, setQuery] = useState("");
  const [workspacePath, setWorkspacePath] = useState(ALL_WORKSPACES);
  const [sort, setSort] = useState<ArchivedSessionSort>("archived-desc");
  const [restoringId, setRestoringId] = useState<string>();
  const [deleteTarget, setDeleteTarget] = useState<DeleteTarget>();
  const [deleting, setDeleting] = useState(false);
  const [reloadKey, setReloadKey] = useState(0);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(undefined);
    void listArchivedSessions()
      .then((items) => {
        if (active) setSessions(items.filter((session) => session.archived));
      })
      .catch((loadError) => {
        if (active) setError(conciseError(loadError));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [reloadKey]);

  useEffect(() => {
    if (!deleteTarget) return;
    const handleKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      if (!deleting) setDeleteTarget(undefined);
    };
    document.addEventListener("keydown", handleKeyDown, true);
    return () => document.removeEventListener("keydown", handleKeyDown, true);
  }, [deleteTarget, deleting]);

  const untitledLabel = t("conversation.new");
  const unknownWorkspaceLabel = t("settings.archivedUnknownWorkspace");
  const locale = getLanguage() === "zh" ? "zh-CN" : "en";

  const workspaceOptions = useMemo(() => {
    const paths = new Set(
      sessions.map((session) =>
        archivedSessionPath(session, unknownWorkspaceLabel),
      ),
    );
    return [
      { value: ALL_WORKSPACES, label: t("settings.archivedAllWorkspaces") },
      ...[...paths]
        .sort((left, right) => left.localeCompare(right, locale))
        .map((path) => ({ value: path, label: path })),
    ];
  }, [locale, sessions, unknownWorkspaceLabel]);

  useEffect(() => {
    if (
      loading ||
      workspacePath === ALL_WORKSPACES ||
      workspaceOptions.some((option) => option.value === workspacePath)
    ) {
      return;
    }
    setWorkspacePath(ALL_WORKSPACES);
  }, [loading, workspaceOptions, workspacePath]);

  const filtered = useMemo(
    () =>
      filterArchivedSessions(sessions, {
        query,
        workspacePath,
        sort,
        untitledLabel,
        unknownWorkspaceLabel,
        locale,
      }),
    [
      locale,
      query,
      sessions,
      sort,
      unknownWorkspaceLabel,
      untitledLabel,
      workspacePath,
    ],
  );
  const groups = useMemo(
    () => groupArchivedSessions(filtered, unknownWorkspaceLabel),
    [filtered, unknownWorkspaceLabel],
  );
  const mutationBusy = restoringId !== undefined || deleting;

  const handleRestore = async (sessionId: string): Promise<void> => {
    if (mutationBusy) return;
    setRestoringId(sessionId);
    setError(undefined);
    try {
      await restoreSession(sessionId);
      setSessions((current) => removeArchivedSessions(current, [sessionId]));
    } catch (restoreError) {
      setError(conciseError(restoreError));
    } finally {
      setRestoringId(undefined);
    }
  };

  const requestWorkspaceDelete = (
    workspaceId: string,
    path: string,
  ): void => {
    if (mutationBusy) return;
    const sessionIds = archivedSessionIdsForWorkspace(sessions, workspaceId);
    if (sessionIds.length === 0) return;
    setDeleteTarget({ kind: "workspace", sessionIds, label: path });
  };

  const handleDelete = async (): Promise<void> => {
    if (!deleteTarget || mutationBusy) return;
    setDeleting(true);
    setError(undefined);
    try {
      const deletedSessionIds = await deleteArchivedSessions(
        deleteTarget.sessionIds,
      );
      setSessions((current) =>
        removeArchivedSessions(current, deletedSessionIds),
      );
      setDeleteTarget(undefined);
    } catch (deleteError) {
      const message = conciseError(deleteError);
      try {
        const items = await listArchivedSessions();
        setSessions(items.filter((session) => session.archived));
      } catch {
        // Preserve the original mutation error; Retry performs a full reload.
      }
      setError(message);
    } finally {
      setDeleting(false);
    }
  };

  return (
    <section
      className="settings-archived"
      aria-labelledby="archived-sessions-heading"
    >
      <header className="settings-archived-header">
        <h3 id="archived-sessions-heading">
          {t("settings.archivedTitle")}
        </h3>
        <p>{t("settings.archivedDescription")}</p>
      </header>

      <div className="settings-archived-toolbar">
        <label className="settings-archived-search">
          <Search size={15} aria-hidden="true" />
          <input
            type="search"
            value={query}
            placeholder={t("settings.archivedSearch")}
            aria-label={t("settings.archivedSearch")}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <SettingsSelect
          className="settings-archived-workspace-select"
          value={workspacePath}
          options={workspaceOptions}
          ariaLabel={t("settings.archivedAllWorkspaces")}
          onChange={setWorkspacePath}
        />
      </div>

      <div
        className="settings-segmented settings-archived-sort"
        role="group"
        aria-label={t("settings.archivedSortLabel")}
      >
        <button
          className={sort === "archived-desc" ? "active" : ""}
          type="button"
          aria-pressed={sort === "archived-desc"}
          onClick={() => setSort("archived-desc")}
        >
          <Clock3 size={14} />
          {t("settings.archivedSortArchived")}
        </button>
        <button
          className={sort === "created-desc" ? "active" : ""}
          type="button"
          aria-pressed={sort === "created-desc"}
          onClick={() => setSort("created-desc")}
        >
          <CalendarDays size={14} />
          {t("settings.archivedSortCreated")}
        </button>
        <button
          className={sort === "name-asc" ? "active" : ""}
          type="button"
          aria-pressed={sort === "name-asc"}
          onClick={() => setSort("name-asc")}
        >
          <ListOrdered size={14} />
          {t("settings.archivedSortName")}
        </button>
      </div>

      {error && (
        <div className="settings-archived-error" role="alert">
          <span>{error}</span>
          <button type="button" onClick={() => setReloadKey((value) => value + 1)}>
            <RefreshCw size={13} />
            {t("common.retry")}
          </button>
        </div>
      )}

      {loading ? (
        <div className="settings-archived-empty" role="status">
          {t("settings.archivedLoading")}
        </div>
      ) : groups.length > 0 ? (
        <div className="settings-archived-list">
          {groups.map((group) => (
            <section className="settings-archived-group" key={group.workspaceId}>
              <div className="settings-archived-workspace">
                <Folder size={15} aria-hidden="true" />
                <span title={group.path}>{group.path}</span>
                <small>
                  {t("settings.archivedCount", {
                    count: group.sessions.length,
                  })}
                </small>
                <button
                  className="settings-archived-delete-all"
                  type="button"
                  disabled={mutationBusy}
                  aria-label={t("settings.archivedDeleteAllLabel", {
                    path: group.path,
                  })}
                  onClick={() =>
                    requestWorkspaceDelete(group.workspaceId, group.path)
                  }
                >
                  <Trash2 size={13} />
                  {t("settings.archivedDeleteAll")}
                </button>
              </div>
              <div className="settings-archived-card">
                {group.sessions.map((session) => {
                  const title = archivedSessionTitle(session, untitledLabel);
                  return (
                    <div className="settings-archived-row" key={session.id}>
                      <div className="settings-archived-meta">
                        <strong title={title}>{title}</strong>
                        <span>
                          {t("settings.archivedAt", {
                            time: formatArchivedTime(session.updatedAt),
                          })}
                        </span>
                      </div>
                      <div className="settings-archived-actions">
                        <button
                          className="settings-archived-restore"
                          type="button"
                          disabled={mutationBusy}
                          onClick={() => void handleRestore(session.id)}
                        >
                          <RotateCcw
                            size={14}
                            className={
                              restoringId === session.id ? "spinning" : undefined
                            }
                          />
                          {restoringId === session.id
                            ? t("settings.archivedRestoring")
                            : t("settings.archivedRestore")}
                        </button>
                        <button
                          className="settings-archived-delete"
                          type="button"
                          disabled={mutationBusy}
                          onClick={() =>
                            setDeleteTarget({
                              kind: "session",
                              sessionIds: [session.id],
                              label: title,
                            })
                          }
                        >
                          <Trash2 size={14} />
                          {t("settings.archivedDelete")}
                        </button>
                      </div>
                    </div>
                  );
                })}
              </div>
            </section>
          ))}
        </div>
      ) : (
        <div className="settings-archived-empty">
          {sessions.length === 0
            ? t("settings.archivedEmpty")
            : t("settings.archivedNoMatch")}
        </div>
      )}

      {deleteTarget && (
        <div
          className="settings-archived-confirm-backdrop"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget && !deleting) {
              setDeleteTarget(undefined);
            }
          }}
        >
          <div
            className="settings-archived-confirm"
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="archived-delete-confirm-title"
          >
            <span className="settings-archived-confirm-icon">
              <Trash2 size={19} />
            </span>
            <h4 id="archived-delete-confirm-title">
              {t(
                deleteTarget.kind === "session"
                  ? "settings.archivedDeleteSessionTitle"
                  : "settings.archivedDeleteWorkspaceTitle",
              )}
            </h4>
            <p>
              {deleteTarget.kind === "session"
                ? t("settings.archivedDeleteSessionCopy", {
                    title: deleteTarget.label,
                  })
                : t("settings.archivedDeleteWorkspaceCopy", {
                    path: deleteTarget.label,
                    count: deleteTarget.sessionIds.length,
                  })}
            </p>
            <div className="settings-archived-confirm-actions">
              <button
                type="button"
                autoFocus
                disabled={deleting}
                onClick={() => setDeleteTarget(undefined)}
              >
                {t("common.cancel")}
              </button>
              <button
                className="danger"
                type="button"
                disabled={deleting}
                onClick={() => void handleDelete()}
              >
                {deleting ? t("settings.archivedDeleting") : t("settings.archivedConfirmDelete")}
              </button>
            </div>
          </div>
        </div>
      )}
    </section>
  );
}
