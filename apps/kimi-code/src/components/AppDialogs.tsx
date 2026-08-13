import { type FormEvent, useCallback, useEffect, useState } from "react";
import {
  Archive,
  ArrowUp,
  Check,
  ChevronRight,
  Copy,
  ExternalLink,
  Folder,
  FolderMinus,
  ShieldCheck,
  Sparkles,
  Target,
  Undo2,
  X,
} from "lucide-react";

import { t } from "../i18n";
import { invoke, openExternalUrl } from "../transport";
import type { DeviceCode, GoalSnapshot } from "../types";
import { conciseError } from "../utils/errors";

const MAX_GOAL_OBJECTIVE_LENGTH = 4_000;

export type RemovalTarget =
  | {
      kind: "project";
      projectId: string;
      name: string;
      path: string;
      conversationIds: string[];
    }
  | {
      kind: "conversation";
      projectId: string;
      conversationId: string;
      title: string;
    };

interface FolderHome {
  home: string;
  recent_roots: string[];
}

interface FolderBrowse {
  path: string;
  parent: string | null;
  entries: Array<{ name: string; path: string; is_dir: true }>;
}

export function GoalEditDialog({
  goal,
  busy,
  onClose,
  onConfirm,
}: {
  goal: GoalSnapshot;
  busy: boolean;
  onClose: () => void;
  onConfirm: (objective: string) => void;
}) {
  const [objective, setObjective] = useState(goal.objective);
  const trimmed = objective.trim();
  const changed = trimmed !== goal.objective.trim();

  useEffect(() => {
    const closeOnEscape = (event: globalThis.KeyboardEvent): void => {
      if (event.key === "Escape" && !busy) onClose();
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [busy, onClose]);

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <section
        className="operation-dialog goal-edit-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="goal-edit-dialog-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <button
          className="dialog-close"
          type="button"
          aria-label={t("goal.editClose")}
          onClick={onClose}
          disabled={busy}
        >
          <X size={17} />
        </button>
        <div className="operation-dialog-icon goal">
          <Target size={23} />
        </div>
        <p className="eyebrow">GOAL</p>
        <h2 id="goal-edit-dialog-title">{t("goal.editTitle")}</h2>
        <p className="dialog-copy">{t("goal.editCopy")}</p>
        <form
          className="goal-edit-form"
          onSubmit={(event) => {
            event.preventDefault();
            if (trimmed && changed && !busy) onConfirm(trimmed);
          }}
        >
          <label htmlFor="goal-edit-objective">{t("goal.objective")}</label>
          <textarea
            id="goal-edit-objective"
            value={objective}
            maxLength={MAX_GOAL_OBJECTIVE_LENGTH}
            rows={5}
            autoFocus
            disabled={busy}
            placeholder={t("goal.editPlaceholder")}
            onChange={(event) => setObjective(event.target.value)}
          />
          <small className="goal-edit-count">
            {objective.length}/{MAX_GOAL_OBJECTIVE_LENGTH}
          </small>
          <div className="operation-dialog-actions">
            <button
              className="dialog-secondary"
              type="button"
              onClick={onClose}
              disabled={busy}
            >
              {t("common.cancel")}
            </button>
            <button
              className="dialog-primary"
              type="submit"
              disabled={!trimmed || !changed || busy}
            >
              {busy ? (
                <>
                  <span className="spinner light" />
                  {t("common.processing")}
                </>
              ) : (
                <>
                  <Check size={15} />
                  {t("goal.save")}
                </>
              )}
            </button>
          </div>
        </form>
      </section>
    </div>
  );
}

export function UndoMessageDialog({
  busy,
  onClose,
  onConfirm,
}: {
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  useEffect(() => {
    const closeOnEscape = (event: globalThis.KeyboardEvent): void => {
      if (event.key === "Escape" && !busy) onClose();
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [busy, onClose]);

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <section
        className="operation-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="undo-message-dialog-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <button
          className="dialog-close"
          type="button"
          aria-label={t("undo.close")}
          onClick={onClose}
          disabled={busy}
        >
          <X size={17} />
        </button>
        <div className="operation-dialog-icon conversation">
          <Undo2 size={22} />
        </div>
        <p className="eyebrow">CONVERSATION UNDO</p>
        <h2 id="undo-message-dialog-title">{t("undo.title")}</h2>
        <p className="dialog-copy">{t("undo.confirm")}</p>
        <div className="operation-dialog-actions">
          <button
            className="dialog-secondary"
            type="button"
            onClick={onClose}
            disabled={busy}
            autoFocus
          >
            {t("common.cancel")}
          </button>
          <button
            className="dialog-danger"
            type="button"
            onClick={onConfirm}
            disabled={busy}
          >
            {busy ? (
              <>
                <span className="spinner light" />
                {t("common.processing")}
              </>
            ) : (
              <>
                <Undo2 size={15} />
                {t("undo.action")}
              </>
            )}
          </button>
        </div>
      </section>
    </div>
  );
}

export function RemovalDialog({
  target,
  busy,
  onClose,
  onConfirm,
}: {
  target: RemovalTarget;
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const isProject = target.kind === "project";

  useEffect(() => {
    const closeOnEscape = (event: globalThis.KeyboardEvent): void => {
      if (event.key === "Escape" && !busy) onClose();
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [busy, onClose]);

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <section
        className="operation-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="removal-dialog-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <button
          className="dialog-close"
          type="button"
          aria-label={t("removal.close")}
          onClick={onClose}
          disabled={busy}
        >
          <X size={17} />
        </button>
        <div
          className={`operation-dialog-icon ${
            isProject ? "project" : "conversation"
          }`}
        >
          {isProject ? <FolderMinus size={23} /> : <Archive size={22} />}
        </div>
        <p className="eyebrow">
          {isProject ? "WORKSPACE CATALOG" : "CONVERSATION ARCHIVE"}
        </p>
        <h2 id="removal-dialog-title">
          {isProject ? t("removal.projectTitle") : t("removal.conversationTitle")}
        </h2>
        {!isProject && (
          <div
            className="operation-target conversation-title"
            title={target.title}
          >
            <span>{target.title}</span>
          </div>
        )}
        <p className="dialog-copy">
          {isProject
            ? t("removal.projectCopy", { name: target.name })
            : t("removal.conversationCopy")}
        </p>
        {isProject && <div className="operation-target">{target.path}</div>}
        <div className="operation-dialog-actions">
          <button
            className="dialog-secondary"
            type="button"
            onClick={onClose}
            disabled={busy}
            autoFocus
          >
            {t("common.cancel")}
          </button>
          <button
            className="dialog-danger"
            type="button"
            onClick={onConfirm}
            disabled={busy}
          >
            {busy ? (
              <>
                <span className="spinner light" />
                {t("common.processing")}
              </>
            ) : isProject ? (
              <>
                <FolderMinus size={15} />
                {t("sidebar.removeProject")}
              </>
            ) : (
              <>
                <Archive size={15} />
                {t("conversation.archive")}
              </>
            )}
          </button>
        </div>
      </section>
    </div>
  );
}

export function DirectoryPickerDialog({
  onClose,
  onSelect,
}: {
  onClose: () => void;
  onSelect: (path: string) => void;
}) {
  const [home, setHome] = useState<FolderHome>();
  const [browse, setBrowse] = useState<FolderBrowse>();
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<string>();

  const navigate = useCallback(async (target?: string): Promise<void> => {
    setBusy(true);
    setError(undefined);
    try {
      const result = await invoke<FolderBrowse>("fs_browse", {
        ...(target ? { path: target } : {}),
      });
      setBrowse(result);
      setPath(result.path);
    } catch (cause) {
      setError(conciseError(cause));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    let active = true;
    void invoke<FolderHome>("fs_home")
      .then((result) => {
        if (!active) return;
        setHome(result);
        return navigate(result.home);
      })
      .catch((cause) => {
        if (active) {
          setError(conciseError(cause));
          setBusy(false);
        }
      });
    return () => {
      active = false;
    };
  }, [navigate]);

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <section
        className="directory-picker-dialog"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <button className="dialog-close" aria-label={t("login.close")} onClick={onClose}>
          <X size={17} />
        </button>
        <p className="eyebrow">KIMI CODE WEB</p>
        <h2>{t("folderPicker.title")}</h2>
        <form
          className="directory-path-form"
          onSubmit={(event) => {
            event.preventDefault();
            void navigate(path.trim());
          }}
        >
          <input
            value={path}
            onChange={(event) => setPath(event.target.value)}
            aria-label={t("folderPicker.path")}
          />
          <button type="submit" className="dialog-secondary" disabled={!path.trim() || busy}>
            {t("folderPicker.go")}
          </button>
        </form>
        {home && (
          <div className="directory-roots">
            {[home.home, ...home.recent_roots]
              .filter((item, index, values) => values.indexOf(item) === index)
              .map((root) => (
                <button type="button" key={root} onClick={() => void navigate(root)}>
                  <Folder size={13} />
                  <span>{root}</span>
                </button>
              ))}
          </div>
        )}
        <div className="directory-list">
          {busy ? (
            <div className="history-loading"><span className="spinner" />{t("folderPicker.loading")}</div>
          ) : error ? (
            <div className="history-loading error">{error}</div>
          ) : (
            <>
              {browse?.parent && (
                <button type="button" onClick={() => void navigate(browse.parent ?? undefined)}>
                  <Folder size={16} />
                  <span>..</span>
                  <ChevronRight size={15} />
                </button>
              )}
              {browse?.entries.map((entry) => (
                <button type="button" key={entry.path} onClick={() => void navigate(entry.path)}>
                  <Folder size={16} />
                  <span>{entry.name}</span>
                  <ChevronRight size={15} />
                </button>
              ))}
            </>
          )}
        </div>
        <div className="operation-dialog-actions">
          <button className="dialog-secondary" type="button" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            className="dialog-primary"
            type="button"
            disabled={!browse?.path || busy}
            onClick={() => browse?.path && onSelect(browse.path)}
          >
            {t("folderPicker.select")}
          </button>
        </div>
      </section>
    </div>
  );
}

export function LoginDialog({
  busy,
  code,
  onClose,
  onStart,
}: {
  busy: boolean;
  code?: DeviceCode;
  onClose: () => void;
  onStart: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const copyCode = async (): Promise<void> => {
    if (!code) return;
    await navigator.clipboard.writeText(code.userCode);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };
  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <section className="login-dialog" onMouseDown={(event) => event.stopPropagation()}>
        <button
          className="dialog-close"
          aria-label={t("login.close")}
          onClick={onClose}
          disabled={busy}
        >
          <X size={17} />
        </button>
        <div className="login-logo">
          <Sparkles size={24} />
        </div>
        <p className="eyebrow">KIMI CODE ACCOUNT</p>
        <h2>{t("login.title")}</h2>
        <p className="dialog-copy">
          {t("login.copy")}
        </p>
        {code ? (
          <>
            <div className="device-code">
              <span>{t("login.deviceCode")}</span>
              <strong>{code.userCode}</strong>
              <button
                className="device-code-copy"
                type="button"
                title={copied ? t("common.copied") : t("common.copy")}
                aria-label={copied ? t("common.copied") : t("common.copy")}
                onClick={() => void copyCode()}
              >
                {copied ? <Check size={14} /> : <Copy size={14} />}
              </button>
            </div>
            <button
              className="dialog-primary"
              onClick={() =>
                void openExternalUrl(
                  code.verificationUriComplete || code.verificationUri,
                )
              }
            >
              {t("login.authorize")}
              <ExternalLink size={16} />
            </button>
            <div className="waiting-line">
              <span className="spinner" />
              {t("login.waiting")}
            </div>
          </>
        ) : (
          <>
            <div className="login-features">
              <span><Check size={14} /> {t("login.featureOauth")}</span>
              <span><Check size={14} /> {t("login.featureSync")}</span>
              <span><Check size={14} /> {t("login.featureLocal")}</span>
            </div>
            <button className="dialog-primary" onClick={onStart} disabled={busy}>
              {busy ? (
                <>
                  <span className="spinner light" />
                  {t("login.creating")}
                </>
              ) : (
                <>
                  {t("login.continue")}
                  <ArrowUp size={16} />
                </>
              )}
            </button>
          </>
        )}
      </section>
    </div>
  );
}

export function WebCredentialDialog({
  onSubmit,
}: {
  onSubmit: (credential: string) => void;
}) {
  const [credential, setCredential] = useState("");
  const submit = (event: FormEvent): void => {
    event.preventDefault();
    const value = credential.trim();
    if (value) onSubmit(value);
  };
  return (
    <div className="modal-backdrop">
      <form
        className="login-dialog"
        onSubmit={submit}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="login-logo">
          <ShieldCheck size={24} />
        </div>
        <p className="eyebrow">KIMI CODE WEB</p>
        <h2>{t("webAuth.title")}</h2>
        <p className="dialog-copy">{t("webAuth.description")}</p>
        <input
          className="web-credential-input"
          type="password"
          autoFocus
          autoComplete="off"
          value={credential}
          placeholder={t("webAuth.placeholder")}
          onChange={(event) => setCredential(event.target.value)}
        />
        <button className="dialog-primary" type="submit" disabled={!credential.trim()}>
          {t("webAuth.connect")}
          <ArrowUp size={16} />
        </button>
      </form>
    </div>
  );
}
