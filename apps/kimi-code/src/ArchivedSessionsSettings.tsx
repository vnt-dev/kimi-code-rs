import {
  CalendarDays,
  Clock3,
  Folder,
  ListOrdered,
  RefreshCw,
  RotateCcw,
  Search,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { listArchivedSessions, restoreSession } from "./agentRpc";
import {
  archivedSessionPath,
  archivedSessionTitle,
  filterArchivedSessions,
  formatArchivedTime,
  groupArchivedSessions,
  type ArchivedSessionSort,
} from "./archivedSessions";
import SettingsSelect from "./components/SettingsSelect";
import { getLanguage, t } from "./i18n";
import type { SessionSummary } from "./types";
import { conciseError } from "./utils/errors";

const ALL_WORKSPACES = "all";

export default function ArchivedSessionsSettings() {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [query, setQuery] = useState("");
  const [workspacePath, setWorkspacePath] = useState(ALL_WORKSPACES);
  const [sort, setSort] = useState<ArchivedSessionSort>("archived-desc");
  const [restoringId, setRestoringId] = useState<string>();
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

  const handleRestore = async (sessionId: string): Promise<void> => {
    if (restoringId) return;
    setRestoringId(sessionId);
    setError(undefined);
    try {
      await restoreSession(sessionId);
      setSessions((current) =>
        current.filter((session) => session.id !== sessionId),
      );
    } catch (restoreError) {
      setError(conciseError(restoreError));
    } finally {
      setRestoringId(undefined);
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
            <section className="settings-archived-group" key={group.path}>
              <div className="settings-archived-workspace">
                <Folder size={15} aria-hidden="true" />
                <span title={group.path}>{group.path}</span>
                <small>
                  {t("settings.archivedCount", {
                    count: group.sessions.length,
                  })}
                </small>
              </div>
              <div className="settings-archived-card">
                {group.sessions.map((session) => (
                  <div className="settings-archived-row" key={session.id}>
                    <div className="settings-archived-meta">
                      <strong title={archivedSessionTitle(session, untitledLabel)}>
                        {archivedSessionTitle(session, untitledLabel)}
                      </strong>
                      <span>
                        {t("settings.archivedAt", {
                          time: formatArchivedTime(session.updatedAt),
                        })}
                      </span>
                    </div>
                    <button
                      className="settings-archived-restore"
                      type="button"
                      disabled={restoringId !== undefined}
                      onClick={() => void handleRestore(session.id)}
                    >
                      <RotateCcw
                        size={14}
                        className={restoringId === session.id ? "spinning" : undefined}
                      />
                      {restoringId === session.id
                        ? t("settings.archivedRestoring")
                        : t("settings.archivedRestore")}
                    </button>
                  </div>
                ))}
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
    </section>
  );
}
