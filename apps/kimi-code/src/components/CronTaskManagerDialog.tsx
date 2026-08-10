import {
  AlertCircle,
  BellRing,
  CalendarClock,
  Check,
  Clock3,
  Plus,
  RefreshCw,
  Repeat2,
  Trash2,
  X,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
} from "react";

import {
  CRON_QUICK_PRESETS,
  formatCronDate,
  type CreateCronTaskInput,
  type CronTaskDescriptor,
  type DeleteCronTaskInput,
} from "../cronTasks";
import { t } from "../i18n";
import { invoke } from "../transport";

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function CronTaskManagerDialog({
  sessionId,
  onCountChange,
  onClose,
}: {
  sessionId: string;
  onCountChange: (count: number) => void;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const onCountChangeRef = useRef(onCountChange);
  const [tasks, setTasks] = useState<CronTaskDescriptor[]>([]);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [deletingId, setDeletingId] = useState<string>();
  const [confirmDeleteId, setConfirmDeleteId] = useState<string>();
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [createOpen, setCreateOpen] = useState(false);
  const [recurring, setRecurring] = useState(true);
  const [cron, setCron] = useState("*/15 * * * *");
  const [prompt, setPrompt] = useState("");

  useEffect(() => {
    onCountChangeRef.current = onCountChange;
  }, [onCountChange]);

  const loadTasks = useCallback(
    async (quiet = false): Promise<void> => {
      if (!quiet) setRefreshing(true);
      try {
        const next = await invoke<CronTaskDescriptor[]>("list_cron_tasks", {
          sessionId,
        });
        setTasks(next);
        onCountChangeRef.current(next.length);
        if (!quiet) setError(undefined);
      } catch (nextError) {
        if (!quiet) setError(errorMessage(nextError));
      } finally {
        setLoading(false);
        if (!quiet) setRefreshing(false);
      }
    },
    [sessionId],
  );

  useEffect(() => {
    void loadTasks();
  }, [loadTasks]);

  useEffect(() => {
    dialogRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      if (confirmDeleteId) {
        setConfirmDeleteId(undefined);
      } else if (createOpen) {
        setCreateOpen(false);
      } else if (!saving && !deletingId) {
        onClose();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [confirmDeleteId, createOpen, deletingId, onClose, saving]);

  const createTask = async (event: FormEvent): Promise<void> => {
    event.preventDefault();
    const normalizedCron = cron.trim().replace(/\s+/g, " ");
    const normalizedPrompt = prompt.trim();
    if (!normalizedCron || !normalizedPrompt) {
      setError(t("cron.validationRequired"));
      return;
    }
    setSaving(true);
    setError(undefined);
    setNotice(undefined);
    try {
      const input: CreateCronTaskInput = {
        sessionId,
        cron: normalizedCron,
        prompt: normalizedPrompt,
        recurring,
      };
      const created = await invoke<CronTaskDescriptor>("create_cron_task", {
        input,
      });
      setTasks((current) => {
        const next = [...current, created];
        onCountChangeRef.current(next.length);
        return next;
      });
      setPrompt("");
      setCreateOpen(false);
      setNotice(t("cron.created", { schedule: created.humanSchedule }));
    } catch (nextError) {
      setError(errorMessage(nextError));
    } finally {
      setSaving(false);
    }
  };

  const deleteTask = async (task: CronTaskDescriptor): Promise<void> => {
    setDeletingId(task.id);
    setError(undefined);
    setNotice(undefined);
    try {
      const input: DeleteCronTaskInput = { sessionId, id: task.id };
      await invoke<void>("delete_cron_task", { input });
      setTasks((current) => {
        const next = current.filter((candidate) => candidate.id !== task.id);
        onCountChangeRef.current(next.length);
        return next;
      });
      setConfirmDeleteId(undefined);
      setNotice(t("cron.deleted"));
    } catch (nextError) {
      setError(errorMessage(nextError));
    } finally {
      setDeletingId(undefined);
    }
  };

  return (
    <div
      className="cron-manager-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !saving && !deletingId) {
          onClose();
        }
      }}
    >
      <div
        ref={dialogRef}
        className="cron-manager-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="cron-manager-title"
        tabIndex={-1}
      >
        <header className="cron-manager-header">
          <div className="cron-manager-heading">
            <span className="cron-manager-heading-icon">
              <CalendarClock size={19} />
            </span>
            <div>
              <h2 id="cron-manager-title">{t("cron.title")}</h2>
            </div>
          </div>
          <button
            className="cron-manager-close"
            type="button"
            aria-label={t("cron.close")}
            disabled={saving || Boolean(deletingId)}
            onClick={onClose}
          >
            <X size={18} />
          </button>
        </header>

        {(error || notice) && (
          <div className={`cron-manager-banner ${error ? "error" : "success"}`} role={error ? "alert" : "status"}>
            {error ? <AlertCircle size={15} /> : <Check size={15} />}
            <span>{error ?? notice}</span>
            <button type="button" aria-label={t("notice.dismiss")} onClick={() => { setError(undefined); setNotice(undefined); }}>
              <X size={14} />
            </button>
          </div>
        )}

        <div className={`cron-manager-body ${createOpen ? "create-open" : ""}`}>
          <section className="cron-manager-list-pane" aria-label={t("cron.list")}>
            <div className="cron-manager-section-heading">
              <div className="cron-manager-section-copy">
                <strong>{t("cron.current")}</strong>
                <small>{t("cron.currentHint", { count: tasks.length })}</small>
              </div>
              <div className="cron-manager-section-actions">
                <button
                  type="button"
                  title={t("cron.refresh")}
                  aria-label={t("cron.refresh")}
                  disabled={refreshing}
                  onClick={() => void loadTasks()}
                >
                  <RefreshCw className={refreshing ? "spin" : ""} size={15} />
                </button>
                <button
                  className="add"
                  type="button"
                  title={t("cron.add")}
                  aria-label={t("cron.add")}
                  disabled={createOpen}
                  onClick={() => setCreateOpen(true)}
                >
                  <Plus size={15} />
                </button>
              </div>
            </div>

            <div className="cron-manager-list">
              {loading ? (
                <div className="cron-manager-state"><span className="spinner" />{t("cron.loading")}</div>
              ) : tasks.length === 0 ? (
                <div className="cron-manager-empty">
                  <span><CalendarClock size={24} /></span>
                  <strong>{t("cron.empty")}</strong>
                </div>
              ) : (
                tasks.map((task) => {
                  const nextFire = formatCronDate(task.nextFireAt);
                  const confirming = confirmDeleteId === task.id;
                  return (
                    <article className={`cron-manager-task ${task.stale ? "stale" : ""}`} key={task.id}>
                      <div className="cron-manager-task-topline">
                        <span className="cron-manager-task-icon">
                          {task.recurring ? <Repeat2 size={15} /> : <BellRing size={15} />}
                        </span>
                        <div className="cron-manager-task-title">
                          <strong>{task.humanSchedule}</strong>
                          <code>{task.cron}</code>
                        </div>
                        <span className={`cron-manager-kind ${task.recurring ? "recurring" : "once"}`}>
                          {task.recurring ? t("cron.recurring") : t("cron.once")}
                        </span>
                      </div>
                      <p className="cron-manager-task-prompt">{task.prompt}</p>
                      <div className="cron-manager-task-meta">
                        <span><Clock3 size={13} />{nextFire ? t("cron.next", { time: nextFire }) : t("cron.nextUnknown")}</span>
                        <span title={task.id}>#{task.id.slice(-6)}</span>
                      </div>
                      {task.stale && <div className="cron-manager-stale"><AlertCircle size={13} />{t("cron.stale")}</div>}
                      {confirming ? (
                        <div className="cron-manager-delete-confirm">
                          <span>{t("cron.deleteQuestion")}</span>
                          <button type="button" disabled={Boolean(deletingId)} onClick={() => setConfirmDeleteId(undefined)}>{t("cron.cancel")}</button>
                          <button className="danger" type="button" disabled={Boolean(deletingId)} onClick={() => void deleteTask(task)}>
                            {deletingId === task.id ? t("cron.deleting") : t("cron.deleteConfirm")}
                          </button>
                        </div>
                      ) : (
                        <button
                          className="cron-manager-delete"
                          type="button"
                          title={t("cron.delete")}
                          aria-label={t("cron.deleteNamed", { schedule: task.humanSchedule })}
                          onClick={() => setConfirmDeleteId(task.id)}
                        >
                          <Trash2 size={14} />
                        </button>
                      )}
                    </article>
                  );
                })
              )}
            </div>
          </section>

          {createOpen && <form className="cron-manager-create-pane" onSubmit={(event) => void createTask(event)}>
            <div className="cron-manager-section-heading">
              <div className="cron-manager-section-copy">
                <strong>{t("cron.add")}</strong>
                <small>{t("cron.addHint")}</small>
              </div>
              <button type="button" title={t("cron.closeCreate")} aria-label={t("cron.closeCreate")} onClick={() => setCreateOpen(false)}>
                <X size={15} />
              </button>
            </div>

            <label className="cron-manager-label">
              <span>{t("cron.type")}</span>
              <div className="cron-manager-type-switch">
                <button className={recurring ? "active" : ""} type="button" onClick={() => setRecurring(true)}>
                  <Repeat2 size={14} />{t("cron.recurring")}
                </button>
                <button className={!recurring ? "active" : ""} type="button" onClick={() => setRecurring(false)}>
                  <BellRing size={14} />{t("cron.once")}
                </button>
              </div>
            </label>

            <label className="cron-manager-label">
              <span>{t("cron.expression")}</span>
              <input value={cron} onChange={(event) => setCron(event.target.value)} placeholder="*/15 * * * *" spellCheck={false} />
              <small>{t("cron.expressionHint")}</small>
            </label>

            <div className="cron-manager-presets" aria-label={t("cron.presets")}>
              {CRON_QUICK_PRESETS.map((preset) => (
                <button key={preset.key} className={cron === preset.cron ? "active" : ""} type="button" onClick={() => setCron(preset.cron)}>
                  {t(`cron.preset.${preset.key}`)}
                </button>
              ))}
            </div>

            <label className="cron-manager-label cron-manager-prompt-label">
              <span>{t("cron.prompt")}</span>
              <textarea value={prompt} onChange={(event) => setPrompt(event.target.value)} placeholder={t("cron.promptPlaceholder")} maxLength={8192} />
              <small>{t("cron.promptHint", { count: prompt.length })}</small>
            </label>

            <div className="cron-manager-create-note">
              <Clock3 size={14} />
              <span>{recurring ? t("cron.recurringNote") : t("cron.onceNote")}</span>
            </div>

            <button className="cron-manager-submit" type="submit" disabled={saving || !cron.trim() || !prompt.trim()}>
              {saving ? <span className="spinner" /> : <Plus size={16} />}
              {saving ? t("cron.creating") : t("cron.create")}
            </button>
          </form>}
        </div>
      </div>
    </div>
  );
}
