import {
  AlertCircle,
  ArrowLeft,
  Bot,
  Check,
  FileCode2,
  FolderGit2,
  MonitorCog,
  Plus,
  RefreshCw,
  Save,
  Trash2,
  X,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";

import {
  customAgentKey,
  newCustomAgentTemplate,
  type CustomAgentDescriptor,
  type CustomAgentScope,
  type DeleteCustomAgentInput,
  type SaveCustomAgentInput,
} from "../customAgents";
import { t } from "../i18n";
import { invoke } from "../transport";

interface DraftAgent {
  scope: CustomAgentScope;
  relativePath?: string;
  content: string;
  originalContent: string;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function CustomAgentManagerDialog({
  workspaceId,
  projectName,
  onClose,
}: {
  workspaceId: string;
  projectName: string;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const [agents, setAgents] = useState<CustomAgentDescriptor[]>([]);
  const [activeScope, setActiveScope] = useState<CustomAgentScope>("project");
  const [selectedKey, setSelectedKey] = useState<string>();
  const [draft, setDraft] = useState<DraftAgent>();
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [mobilePane, setMobilePane] = useState<"list" | "editor">("list");

  const dirty = Boolean(draft && draft.content !== draft.originalContent);
  const scopedAgents = useMemo(
    () => agents.filter((agent) => agent.scope === activeScope),
    [activeScope, agents],
  );
  const selectedAgent = useMemo(
    () => agents.find((agent) => customAgentKey(agent) === selectedKey),
    [agents, selectedKey],
  );

  const loadAgents = useCallback(
    async (
      preferredKey?: string,
      fallbackScope: CustomAgentScope = "project",
    ): Promise<void> => {
      setLoading(true);
      setError(undefined);
      try {
        const next = await invoke<CustomAgentDescriptor[]>(
          "list_custom_agents",
          { workspaceId },
        );
        setAgents(next);
        const preferred = next.find(
          (agent) => customAgentKey(agent) === preferredKey,
        );
        const fallback = next.find((agent) => agent.scope === fallbackScope);
        const selection = preferred ?? fallback;
        if (selection) {
          setActiveScope(selection.scope);
          setSelectedKey(customAgentKey(selection));
          setDraft({
            scope: selection.scope,
            relativePath: selection.relativePath,
            content: selection.content,
            originalContent: selection.content,
          });
        } else {
          setSelectedKey(undefined);
          setDraft(undefined);
        }
      } catch (nextError) {
        setError(errorMessage(nextError));
      } finally {
        setLoading(false);
      }
    },
    [workspaceId],
  );

  useEffect(() => {
    void loadAgents();
  }, [loadAgents]);

  const confirmDiscard = useCallback((): boolean => {
    return !dirty || window.confirm(t("agents.discardChanges"));
  }, [dirty]);

  const requestClose = useCallback((): void => {
    if (busy || !confirmDiscard()) return;
    onClose();
  }, [busy, confirmDiscard, onClose]);

  useEffect(() => {
    const previousFocus = document.activeElement;
    dialogRef.current?.focus();
    return () => {
      if (previousFocus instanceof HTMLElement) previousFocus.focus();
    };
  }, []);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      if (confirmDelete) {
        setConfirmDelete(false);
      } else {
        requestClose();
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [confirmDelete, requestClose]);

  const selectScope = (scope: CustomAgentScope): void => {
    if (scope === activeScope || !confirmDiscard()) return;
    setActiveScope(scope);
    setMobilePane("list");
    setNotice(undefined);
    setError(undefined);
    const first = agents.find((agent) => agent.scope === scope);
    if (!first) {
      setSelectedKey(undefined);
      setDraft(undefined);
      return;
    }
    setSelectedKey(customAgentKey(first));
    setDraft({
      scope: first.scope,
      relativePath: first.relativePath,
      content: first.content,
      originalContent: first.content,
    });
  };

  const selectAgent = (agent: CustomAgentDescriptor): void => {
    const key = customAgentKey(agent);
    if (key === selectedKey) {
      setMobilePane("editor");
      return;
    }
    if (!confirmDiscard()) return;
    setSelectedKey(key);
    setDraft({
      scope: agent.scope,
      relativePath: agent.relativePath,
      content: agent.content,
      originalContent: agent.content,
    });
    setNotice(undefined);
    setError(undefined);
    setConfirmDelete(false);
    setMobilePane("editor");
  };

  const createAgent = (): void => {
    if (!confirmDiscard()) return;
    const content = newCustomAgentTemplate();
    setSelectedKey(undefined);
    setDraft({
      scope: activeScope,
      content,
      originalContent: "",
    });
    setNotice(undefined);
    setError(undefined);
    setConfirmDelete(false);
    setMobilePane("editor");
    window.setTimeout(() => editorRef.current?.focus(), 0);
  };

  const saveAgent = useCallback(async (): Promise<void> => {
    if (!draft || busy) return;
    setBusy(true);
    setError(undefined);
    setNotice(undefined);
    try {
      const input: SaveCustomAgentInput = {
        workspaceId,
        scope: draft.scope,
        relativePath: draft.relativePath,
        content: draft.content,
      };
      const saved = await invoke<CustomAgentDescriptor>("save_custom_agent", {
        input,
      });
      setNotice(t("agents.saved", { name: saved.name }));
      await loadAgents(customAgentKey(saved), saved.scope);
    } catch (nextError) {
      setError(errorMessage(nextError));
    } finally {
      setBusy(false);
    }
  }, [busy, draft, loadAgents, workspaceId]);

  const deleteAgent = async (): Promise<void> => {
    if (!draft?.relativePath || busy) return;
    setBusy(true);
    setError(undefined);
    setNotice(undefined);
    try {
      const input: DeleteCustomAgentInput = {
        workspaceId,
        scope: draft.scope,
        relativePath: draft.relativePath,
      };
      await invoke<void>("delete_custom_agent", { input });
      setConfirmDelete(false);
      setSelectedKey(undefined);
      setDraft(undefined);
      setMobilePane("list");
      setNotice(t("agents.deleted"));
      await loadAgents(undefined, activeScope);
    } catch (nextError) {
      setError(errorMessage(nextError));
    } finally {
      setBusy(false);
    }
  };

  const refreshAgents = (): void => {
    if (!confirmDiscard()) return;
    void loadAgents(selectedKey, activeScope);
  };

  const showAgentList = (): void => {
    if (!confirmDiscard()) return;
    if (!draft?.relativePath) {
      setSelectedKey(undefined);
      setDraft(undefined);
    } else if (selectedAgent) {
      setDraft({
        scope: selectedAgent.scope,
        relativePath: selectedAgent.relativePath,
        content: selectedAgent.content,
        originalContent: selectedAgent.content,
      });
    }
    setError(undefined);
    setNotice(undefined);
    setConfirmDelete(false);
    setMobilePane("list");
  };

  const handleEditorKeyDown = (
    event: ReactKeyboardEvent<HTMLTextAreaElement>,
  ): void => {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      void saveAgent();
      return;
    }
    if (event.key !== "Tab" || !draft) return;
    event.preventDefault();
    const editor = event.currentTarget;
    const start = editor.selectionStart;
    const end = editor.selectionEnd;
    const content = `${draft.content.slice(0, start)}  ${draft.content.slice(end)}`;
    setDraft({ ...draft, content });
    window.setTimeout(() => {
      editor.selectionStart = editor.selectionEnd = start + 2;
    }, 0);
  };

  return (
    <div
      className="agent-manager-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) requestClose();
      }}
    >
      <div
        ref={dialogRef}
        className="agent-manager-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="agent-manager-title"
        tabIndex={-1}
      >
        <header className="agent-manager-header">
          <div className="agent-manager-title">
            <span className="agent-manager-title-icon"><Bot size={18} /></span>
            <div>
              <h2 id="agent-manager-title">{t("agents.title")}</h2>
              <p>{t("agents.subtitle")}</p>
            </div>
          </div>
          <button
            className="agent-manager-close"
            type="button"
            aria-label={t("agents.close")}
            onClick={requestClose}
          >
            <X size={17} />
          </button>
        </header>

        <div className="agent-manager-scope-tabs" role="tablist" aria-label={t("agents.scopes")}>
          <button
            className={activeScope === "app" ? "active" : ""}
            type="button"
            role="tab"
            aria-selected={activeScope === "app"}
            onClick={() => selectScope("app")}
          >
            <MonitorCog size={15} />
            <span>{t("agents.scopeApp")}</span>
            <small>{agents.filter((agent) => agent.scope === "app").length}</small>
          </button>
          <button
            className={activeScope === "project" ? "active" : ""}
            type="button"
            role="tab"
            aria-selected={activeScope === "project"}
            onClick={() => selectScope("project")}
          >
            <FolderGit2 size={15} />
            <span>{t("agents.scopeProject")}</span>
            <em>{projectName}</em>
            <small>{agents.filter((agent) => agent.scope === "project").length}</small>
          </button>
        </div>

        <div className={`agent-manager-body ${mobilePane === "editor" ? "mobile-editor-open" : ""}`}>
          <aside className="agent-manager-list-pane">
            <div className="agent-manager-list-heading">
              <div>
                <strong>{activeScope === "app" ? t("agents.scopeApp") : t("agents.scopeProject")}</strong>
                <small>{activeScope === "app" ? t("agents.scopeAppHint") : t("agents.scopeProjectHint")}</small>
              </div>
              <button type="button" title={t("agents.refresh")} disabled={loading || busy} onClick={refreshAgents}>
                <RefreshCw size={14} className={loading ? "spinning" : undefined} />
              </button>
            </div>

            <div className="agent-manager-list" role="listbox" aria-label={t("agents.list")}>
              {loading ? (
                <div className="agent-manager-list-state"><span className="spinner" />{t("agents.loading")}</div>
              ) : scopedAgents.length === 0 ? (
                <div className="agent-manager-empty">
                  <FileCode2 size={22} />
                  <strong>{t("agents.empty")}</strong>
                  <span>{t("agents.emptyHint")}</span>
                </div>
              ) : (
                scopedAgents.map((agent) => {
                  const active = customAgentKey(agent) === selectedKey;
                  return (
                    <button
                      className={`agent-manager-list-item ${active ? "active" : ""} ${agent.valid ? "" : "invalid"}`}
                      type="button"
                      role="option"
                      aria-selected={active}
                      key={customAgentKey(agent)}
                      onClick={() => selectAgent(agent)}
                    >
                      <span className="agent-manager-agent-icon">{agent.valid ? <Bot size={15} /> : <AlertCircle size={15} />}</span>
                      <span className="agent-manager-agent-copy">
                        <strong>{agent.name}</strong>
                        <small>{agent.valid ? agent.description || agent.relativePath : t("agents.invalid")}</small>
                      </span>
                      {agent.isOverride && <span className="agent-manager-override">{t("agents.override")}</span>}
                    </button>
                  );
                })
              )}
            </div>

            <button className="agent-manager-new" type="button" disabled={busy} onClick={createAgent}>
              <Plus size={15} />
              {t("agents.new")}
            </button>
          </aside>

          <section className="agent-manager-editor-pane">
            {draft ? (
              <>
                <div className="agent-manager-editor-heading">
                  <button
                    className="agent-manager-mobile-back"
                    type="button"
                    aria-label={t("agents.backToList")}
                    onClick={showAgentList}
                  >
                    <ArrowLeft size={18} />
                  </button>
                  <div className="agent-manager-editor-meta">
                    <span>{draft.relativePath ? selectedAgent?.name ?? draft.relativePath : t("agents.untitled")}</span>
                    <small>{draft.relativePath ? selectedAgent?.path : t("agents.newHint")}</small>
                  </div>
                  <div className="agent-manager-editor-state">
                    {dirty ? <span className="dirty">{t("agents.unsaved")}</span> : draft.relativePath ? <span><Check size={12} />{t("agents.savedState")}</span> : null}
                  </div>
                </div>

                {(error || selectedAgent?.error) && (
                  <div className="agent-manager-message error" role="alert">
                    <AlertCircle size={15} />
                    <span>{error ?? selectedAgent?.error}</span>
                  </div>
                )}
                {notice && !error && (
                  <div className="agent-manager-message success" role="status">
                    <Check size={15} />
                    <span>{notice}</span>
                  </div>
                )}

                <textarea
                  ref={editorRef}
                  className="agent-manager-editor"
                  aria-label={t("agents.editor")}
                  value={draft.content}
                  disabled={busy}
                  spellCheck={false}
                  onChange={(event) => {
                    setDraft({ ...draft, content: event.target.value });
                    setError(undefined);
                    setNotice(undefined);
                  }}
                  onKeyDown={handleEditorKeyDown}
                />

                <footer className="agent-manager-editor-footer">
                  <span>{t("agents.editorHint")}</span>
                  <span>{draft.content.split("\n").length} {t("agents.lines")} · {draft.content.length} {t("agents.characters")}</span>
                </footer>

                <div className="agent-manager-actions">
                  {draft.relativePath && (
                    <button className="danger" type="button" disabled={busy} onClick={() => setConfirmDelete(true)}>
                      <Trash2 size={14} />
                      {t("agents.delete")}
                    </button>
                  )}
                  <span />
                  <button type="button" disabled={busy || !dirty} onClick={() => void saveAgent()}>
                    <Save size={14} />
                    {busy ? t("agents.saving") : t("agents.save")}
                  </button>
                </div>
              </>
            ) : (
              <div className="agent-manager-editor-empty">
                <Bot size={30} />
                <h3>{t("agents.selectTitle")}</h3>
                <p>{t("agents.selectHint")}</p>
                <button type="button" onClick={createAgent}><Plus size={14} />{t("agents.new")}</button>
              </div>
            )}
          </section>
        </div>

        {confirmDelete && draft?.relativePath && (
          <div className="agent-manager-confirm-backdrop">
            <div className="agent-manager-confirm" role="alertdialog" aria-modal="true" aria-labelledby="agent-delete-title">
              <span className="agent-manager-confirm-icon"><Trash2 size={18} /></span>
              <h3 id="agent-delete-title">{t("agents.deleteTitle")}</h3>
              <p>{t("agents.deleteCopy", { name: selectedAgent?.name ?? draft.relativePath })}</p>
              <div>
                <button type="button" disabled={busy} onClick={() => setConfirmDelete(false)}>{t("common.cancel")}</button>
                <button className="danger" type="button" disabled={busy} onClick={() => void deleteAgent()}>{t("agents.deleteConfirm")}</button>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
