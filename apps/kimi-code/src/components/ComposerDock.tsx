import {
  ArrowUp,
  Bot,
  BrainCircuit,
  Check,
  ClipboardList,
  Copy,
  FileCode2,
  MessageSquareText,
  Minimize2,
  Package,
  Paperclip,
  Pause,
  Play,
  Plus,
  Server,
  ShieldCheck,
  SquarePen,
  Target,
  X,
} from "lucide-react";
import {
  type ChangeEvent,
  type ClipboardEvent,
  type Dispatch,
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
  type SetStateAction,
} from "react";
import { BACKGROUND_TASK_DETAIL_TAIL } from "../app/appUtils";
import type { PromptDraftUpdater } from "../promptDrafts";
import type { SlashMenuItem } from "../plugins";
import {
  skillDisplayName,
  sortSkillsForAddMenu,
} from "../skillDisplayName";
import { thinkingLevelDescription } from "../modelControls";
import { MAX_PROMPT_ATTACHMENTS } from "../prompt/attachments";
import type { PromptAttachment } from "../chat/liveTurns";
import { formatBytes, formatContext } from "../utils/format";
import { t } from "../i18n";
import type {
  AgentInteraction,
  AgentUsageStatus,
  BackgroundTaskView,
  CompactionEvent,
  ContextUsage,
  GoalSnapshot,
  Model,
  PermissionMode,
  PlanData,
  SkillDescriptor,
  TodoItem,
} from "../types";
import type { McpServerInfo } from "../agentRpc";
import {
  BackgroundTaskProgress,
  ContextUsageIndicator,
  TodoProgress,
  ToolbarSelect,
} from "./ChatHeader";
import {
  ApprovalCard,
  PlanReviewCard,
  QuestionCard,
  RetryConfirmationCard,
  isPlanReviewInteraction,
  isRetryConfirmationInteraction,
} from "./InteractionCards";
import { RemixSparklingLineIcon } from "./RemixSparklingLineIcon";
import { McpStatusPopover } from "./McpStatusPopover";
import type { SkillDetailTarget } from "./sidebars/ChatSidebars";

interface ComposerDockProps {
  queuedMessages?: ReactNode;
  activeAgentScope?: { sessionId: string; agentId: string };
  activeAgentUsage?: AgentUsageStatus;
  activeApproval?: AgentInteraction;
  activeBackgroundTasks: BackgroundTaskView[];
  activeCompaction?: CompactionEvent;
  activeContextUsage?: ContextUsage;
  activeGoal?: GoalSnapshot | null;
  activeGoalMode: boolean;
  activePlan?: PlanData | null;
  activeQuestion?: AgentInteraction;
  activeSwarmMode: boolean;
  activeTodos: TodoItem[];
  attachmentInputRef: RefObject<HTMLInputElement | null>;
  availableSkills: SkillDescriptor[];
  composerAddOpen: boolean;
  composerAddRef: RefObject<HTMLDivElement | null>;
  composerHasContent: boolean;
  effort: string;
  forkCommandBusy: boolean;
  hasBlockingInteraction: boolean;
  isHistoryLoading: boolean;
  isStreaming: boolean;
  modeBusy: boolean;
  mcpStatusBusy: boolean;
  mcpStatusError?: string;
  mcpStatusOpen: boolean;
  mcpServers: readonly McpServerInfo[];
  modelBusy: boolean;
  models: Model[];
  modelsBusy: boolean;
  permissionMode: PermissionMode;
  prompt: string;
  promptAttachments: PromptAttachment[];
  promptCompositionRef: RefObject<boolean>;
  promptSkills: SkillDescriptor[];
  resolvingInteraction?: string;
  selectedModel?: Model;
  showStopButton: boolean;
  skillsBusy: boolean;
  skillsError?: string;
  slashMenuActiveIndex: number;
  slashMenuItems: SlashMenuItem[];
  slashMenuOpen: boolean;
  supportedThinkingLevels: string[];
  textareaRef: RefObject<HTMLTextAreaElement | null>;
  cancelActiveTurn: () => Promise<void>;
  chooseEffort: (level: string) => void;
  chooseModel: (modelId: string) => void;
  choosePermissionMode: (mode: PermissionMode) => void;
  controlActiveGoal: (action: "pause" | "resume" | "cancel") => Promise<void>;
  handleAttachmentInput: (event: ChangeEvent<HTMLInputElement>) => void;
  handlePromptKeyDown: (event: KeyboardEvent<HTMLTextAreaElement>) => void;
  handlePromptPaste: (event: ClipboardEvent<HTMLTextAreaElement>) => void;
  handleSubmit: (event: FormEvent) => void;
  loadAvailableSkills: () => Promise<void>;
  loadBackgroundTaskOutput: (
    scope: { sessionId: string; agentId: string },
    taskId: string,
    tail?: number,
  ) => Promise<void>;
  stopBackgroundTask: (
    scope: { sessionId: string; agentId: string },
    taskId: string,
  ) => Promise<void>;
  openSkillDetail: (skill: SkillDetailTarget) => Promise<void>;
  closeMcpStatus: () => void;
  resolveApproval: (
    interaction: AgentInteraction,
    decision: "approved" | "rejected",
    session?: boolean,
  ) => Promise<void>;
  respondToInteraction: (
    interaction: AgentInteraction,
    response: unknown,
  ) => Promise<void>;
  selectSlashMenuItem: (item: SlashMenuItem) => void;
  selectPromptSkill: (skill: SkillDescriptor) => void;
  setComposerAddOpen: Dispatch<SetStateAction<boolean>>;
  setGoalEditTarget: Dispatch<SetStateAction<GoalSnapshot | undefined>>;
  setPromptAttachments: (
    update: PromptDraftUpdater<PromptAttachment[]>,
    conversationId?: string,
  ) => void;
  setPromptSkills: (
    update: PromptDraftUpdater<SkillDescriptor[]>,
    conversationId?: string,
  ) => void;
  setSlashMenuActiveIndex: Dispatch<SetStateAction<number>>;
  setSlashMenuOpen: Dispatch<SetStateAction<boolean>>;
  syncSlashMenu: (textarea: HTMLTextAreaElement) => void;
  toggleComposerAdd: () => void;
  toggleGoalMode: () => Promise<void>;
  togglePlanMode: () => Promise<void>;
  toggleSwarmMode: () => Promise<void>;
  updatePrompt: (value: string, isComposing?: boolean) => void;
}

export function ComposerDock({
  queuedMessages,
  activeAgentScope,
  activeAgentUsage,
  activeApproval,
  activeBackgroundTasks,
  activeCompaction,
  activeContextUsage,
  activeGoal,
  activeGoalMode,
  activePlan,
  activeQuestion,
  activeSwarmMode,
  activeTodos,
  attachmentInputRef,
  availableSkills,
  composerAddOpen,
  composerAddRef,
  composerHasContent,
  effort,
  forkCommandBusy,
  hasBlockingInteraction,
  isHistoryLoading,
  isStreaming,
  modeBusy,
  mcpStatusBusy,
  mcpStatusError,
  mcpStatusOpen,
  mcpServers,
  modelBusy,
  models,
  modelsBusy,
  permissionMode,
  prompt,
  promptAttachments,
  promptCompositionRef,
  promptSkills,
  resolvingInteraction,
  selectedModel,
  showStopButton,
  skillsBusy,
  skillsError,
  slashMenuActiveIndex,
  slashMenuItems,
  slashMenuOpen,
  supportedThinkingLevels,
  textareaRef,
  cancelActiveTurn,
  chooseEffort,
  chooseModel,
  choosePermissionMode,
  controlActiveGoal,
  handleAttachmentInput,
  handlePromptKeyDown,
  handlePromptPaste,
  handleSubmit,
  loadAvailableSkills,
  loadBackgroundTaskOutput,
  stopBackgroundTask,
  openSkillDetail,
  closeMcpStatus,
  resolveApproval,
  respondToInteraction,
  selectSlashMenuItem,
  selectPromptSkill,
  setComposerAddOpen,
  setGoalEditTarget,
  setPromptAttachments,
  setPromptSkills,
  setSlashMenuActiveIndex,
  setSlashMenuOpen,
  syncSlashMenu,
  toggleComposerAdd,
  toggleGoalMode,
  togglePlanMode,
  toggleSwarmMode,
  updatePrompt,
}: ComposerDockProps) {
  return (
            <div className="composer-dock">
              {activeQuestion && (
                isRetryConfirmationInteraction(activeQuestion) ? (
                  <RetryConfirmationCard
                    key={activeQuestion.id}
                    busy={resolvingInteraction === activeQuestion.id}
                    onCancel={() =>
                      void respondToInteraction(activeQuestion, null)
                    }
                    onContinue={(response) =>
                      void respondToInteraction(activeQuestion, response)
                    }
                  />
                ) : (
                  <QuestionCard
                    key={activeQuestion.id}
                    interaction={activeQuestion}
                    busy={resolvingInteraction === activeQuestion.id}
                    onRespond={(response) =>
                      void respondToInteraction(activeQuestion, response)
                    }
                  />
                )
              )}
              {activeApproval && isPlanReviewInteraction(activeApproval) ? (
                <PlanReviewCard
                  key={activeApproval.id}
                  interaction={activeApproval}
                  busy={resolvingInteraction === activeApproval.id}
                  onRespond={(response) =>
                    void respondToInteraction(activeApproval, response)
                  }
                />
              ) : activeApproval ? (
                <ApprovalCard
                  interaction={activeApproval}
                  busy={resolvingInteraction === activeApproval.id}
                  onReject={() =>
                    void resolveApproval(activeApproval, "rejected")
                  }
                  onApprove={() =>
                    void resolveApproval(activeApproval, "approved")
                  }
                  onApproveSession={() =>
                    void resolveApproval(activeApproval, "approved", true)
                  }
                />
              ) : null}
              {(activeBackgroundTasks.length > 0 ||
                activeTodos.some((todo) => todo.status !== "done")) && (
                <div className="composer-progress-row">
                  {activeBackgroundTasks.length > 0 && (
                    <BackgroundTaskProgress
                      tasks={activeBackgroundTasks}
                      onLoadOutput={(taskId) =>
                        activeAgentScope
                          ? loadBackgroundTaskOutput(
                              activeAgentScope,
                              taskId,
                              BACKGROUND_TASK_DETAIL_TAIL,
                            )
                          : Promise.resolve()
                      }
                      onStopTask={(taskId) =>
                        activeAgentScope
                          ? stopBackgroundTask(activeAgentScope, taskId)
                          : Promise.resolve()
                      }
                    />
                  )}
                  {activeTodos.some((todo) => todo.status !== "done") && (
                    <TodoProgress todos={activeTodos} />
                  )}
                </div>
              )}
              {activeGoal && activeGoal.status !== "complete" && (
                <div className="composer-goal-status">
                  <section
                    className={`composer-goal-card ${activeGoal.status}`}
                    aria-label={t("goal.current")}
                  >
                    <span className="composer-goal-icon" aria-hidden="true">
                      <Target size={15} />
                    </span>
                    <span className="composer-goal-copy">
                      <span className="composer-goal-heading">
                        <strong>{t("goal.current")}</strong>
                        <small>
                          {activeGoal.status === "active"
                            ? t("goal.statusActive")
                            : activeGoal.status === "paused"
                              ? t("goal.statusPaused")
                              : t("goal.statusBlocked")}
                        </small>
                      </span>
                      <span
                        className="composer-goal-objective"
                        title={activeGoal.objective}
                      >
                        {activeGoal.objective}
                      </span>
                    </span>
                    <span className="composer-goal-actions">
                      <button
                        className="icon-only"
                        type="button"
                        aria-label={t("goal.editAction")}
                        title={t("goal.editAction")}
                        disabled={modeBusy}
                        onClick={() => setGoalEditTarget(activeGoal)}
                      >
                        <SquarePen size={12} />
                      </button>
                      {activeGoal.status === "active" && (
                        <button
                          className="icon-only"
                          type="button"
                          aria-label={t("goal.pauseAction")}
                          title={t("goal.pauseAction")}
                          disabled={modeBusy}
                          onClick={() => void controlActiveGoal("pause")}
                        >
                          <Pause size={12} />
                        </button>
                      )}
                      {(activeGoal.status === "paused" ||
                        activeGoal.status === "blocked") && (
                        <button
                          className="primary icon-only"
                          type="button"
                          aria-label={t("goal.resumeAction")}
                          title={t("goal.resumeAction")}
                          disabled={modeBusy}
                          onClick={() => void controlActiveGoal("resume")}
                        >
                          <Play size={12} />
                        </button>
                      )}
                      <button
                        className="cancel"
                        type="button"
                        aria-label={t("goal.cancel")}
                        title={t("goal.cancel")}
                        disabled={modeBusy}
                        onClick={() => void controlActiveGoal("cancel")}
                      >
                        <X size={12} />
                      </button>
                    </span>
                  </section>
                </div>
              )}
              {queuedMessages}
              <form className="composer" onSubmit={handleSubmit}>
                {mcpStatusOpen && (
                  <McpStatusPopover
                    servers={mcpServers}
                    busy={mcpStatusBusy}
                    error={mcpStatusError}
                    onClose={closeMcpStatus}
                  />
                )}
                {slashMenuOpen && slashMenuItems.length > 0 && (
                  <div
                    className="slash-command-menu"
                    id="slash-command-menu"
                    role="menu"
                    aria-label={t("slash.commands")}
                    onMouseDown={(event) => event.preventDefault()}
                  >
                    {slashMenuItems.map((item, index) => (
                      <button
                        className={slashMenuActiveIndex === index ? "selected" : undefined}
                        id={`slash-command-${item.id}`}
                        key={item.id}
                        type="button"
                        role="menuitem"
                        disabled={item.disabled}
                        onMouseEnter={() => setSlashMenuActiveIndex(index)}
                        onClick={() => selectSlashMenuItem(item)}
                      >
                        <span className="slash-command-icon" aria-hidden="true">
                          {item.builtin === "compact" ? (
                            activeCompaction?.phase === "started" ? <span className="spinner" /> : <Minimize2 size={14} />
                          ) : item.builtin === "fork" ? (
                            forkCommandBusy ? <span className="spinner" /> : <Copy size={14} />
                          ) : item.builtin === "btw" ? (
                            <MessageSquareText size={14} />
                          ) : item.builtin === "mcp" ? (
                            <Server size={14} />
                          ) : (
                            <Package size={14} />
                          )}
                        </span>
                        <strong>/{item.label}</strong>
                        <small>{item.description}</small>
                      </button>
                    ))}
                  </div>
                )}
                {promptAttachments.length > 0 && (
                  <div className="prompt-attachment-list">
                    {promptAttachments.map((attachment) => (
                      <figure
                        className={`prompt-attachment ${attachment.kind}`}
                        key={attachment.id}
                      >
                        {attachment.kind === "image" ? (
                          <img
                            src={attachment.dataUrl}
                            alt={attachment.name}
                          />
                        ) : attachment.kind === "audio" ? (
                          <audio
                            src={attachment.dataUrl}
                            controls
                            preload="metadata"
                          />
                        ) : attachment.kind === "video" ? (
                          <video
                            src={attachment.dataUrl}
                            controls
                            preload="metadata"
                          />
                        ) : (
                          <div className="prompt-file-preview">
                            <FileCode2 size={24} />
                            <small>{formatBytes(attachment.size)}</small>
                          </div>
                        )}
                        <figcaption title={attachment.name}>
                          {attachment.name}
                        </figcaption>
                        <button
                          type="button"
                          aria-label={t("composer.removeAttachmentNamed", { name: attachment.name })}
                          title={t("composer.removeAttachment")}
                          onClick={() =>
                            setPromptAttachments((current) =>
                              current.filter(
                                (item) => item.id !== attachment.id,
                              ),
                            )
                          }
                        >
                          <X size={12} />
                        </button>
                      </figure>
                    ))}
                  </div>
                )}
                {(
                  activePlan ||
                  activeGoalMode ||
                  activeSwarmMode ||
                  promptSkills.length > 0
                ) && (
                  <div
                    className="prompt-skill-list"
                    aria-label={t("composer.inputSettings")}
                  >
                    {activePlan && (
                      <span className="prompt-skill-chip prompt-plan-chip">
                        <span className="prompt-skill-open prompt-plan-label">
                          <ClipboardList size={13} />
                          <span>{t("plan.label")}</span>
                        </span>
                        <button
                          className="prompt-skill-remove"
                          type="button"
                          aria-label={t("plan.exit")}
                          title={t("plan.exit")}
                          disabled={modeBusy || isStreaming}
                          onClick={() => void togglePlanMode()}
                        >
                          {modeBusy ? (
                            <span className="spinner" />
                          ) : (
                            <X size={11} />
                          )}
                        </button>
                      </span>
                    )}
                    {activeGoalMode && (
                      <span className="prompt-skill-chip prompt-goal-chip">
                        <span className="prompt-skill-open prompt-plan-label">
                          <Target size={13} />
                          <span>{t("goal.label")}</span>
                        </span>
                        <button
                          className="prompt-skill-remove"
                          type="button"
                          aria-label={t("goal.disable")}
                          title={t("goal.disable")}
                          disabled={modeBusy}
                          onClick={() => void toggleGoalMode()}
                        >
                          {modeBusy ? (
                            <span className="spinner" />
                          ) : (
                            <X size={11} />
                          )}
                        </button>
                      </span>
                    )}
                    {activeSwarmMode && (
                      <span className="prompt-skill-chip prompt-plan-chip">
                        <span className="prompt-skill-open prompt-plan-label">
                          <RemixSparklingLineIcon size={13} />
                          <span>{t("swarm.label")}</span>
                        </span>
                        <button
                          className="prompt-skill-remove"
                          type="button"
                          aria-label={t("swarm.disable")}
                          title={t("swarm.disable")}
                          disabled={modeBusy || isStreaming}
                          onClick={() => void toggleSwarmMode()}
                        >
                          {modeBusy ? (
                            <span className="spinner" />
                          ) : (
                            <X size={11} />
                          )}
                        </button>
                      </span>
                    )}
                    {promptSkills.map((skill) => (
                      <span className="prompt-skill-chip" key={skill.name}>
                        <button
                          className="prompt-skill-open"
                          type="button"
                          aria-label={t("skills.viewSkill", { name: skill.name })}
                          title={t("skills.viewDetail")}
                          onClick={() => void openSkillDetail(skill)}
                        >
                          <Package size={13} />
                          <span>{skill.name}</span>
                        </button>
                        <button
                          className="prompt-skill-remove"
                          type="button"
                          aria-label={t("skills.removeSkill", { name: skill.name })}
                          title={t("skills.remove")}
                          onClick={() =>
                            setPromptSkills((current) =>
                              current.filter(
                                (item) => item.name !== skill.name,
                              ),
                            )
                          }
                        >
                          <X size={11} />
                        </button>
                      </span>
                    ))}
                  </div>
                )}
                <input
                  ref={attachmentInputRef}
                  className="prompt-attachment-input"
                  type="file"
                  multiple
                  onChange={handleAttachmentInput}
                />
                <textarea
                  ref={textareaRef}
                  value={prompt}
                  onChange={(event) => {
                    updatePrompt(
                      event.target.value,
                      promptCompositionRef.current ||
                        (event.nativeEvent as InputEvent).isComposing,
                    );
                    syncSlashMenu(event.currentTarget);
                  }}
                  onCompositionStart={() => {
                    promptCompositionRef.current = true;
                  }}
                  onCompositionEnd={() => {
                    promptCompositionRef.current = false;
                    window.requestAnimationFrame(() => {
                      const textarea = textareaRef.current;
                      if (textarea) {
                        updatePrompt(textarea.value);
                        syncSlashMenu(textarea);
                      }
                    });
                  }}
                  onFocus={(event) => syncSlashMenu(event.currentTarget)}
                  onSelect={(event) => syncSlashMenu(event.currentTarget)}
                  onBlur={() => setSlashMenuOpen(false)}
                  onKeyDown={handlePromptKeyDown}
                  onPaste={handlePromptPaste}
                  aria-expanded={slashMenuOpen}
                  aria-controls={
                    slashMenuOpen ? "slash-command-menu" : undefined
                  }
                  aria-activedescendant={
                    slashMenuOpen && slashMenuItems[slashMenuActiveIndex]
                      ? `slash-command-${slashMenuItems[slashMenuActiveIndex].id}`
                      : undefined
                  }
                  placeholder={
                    activePlan
                      ? t("composer.placeholderPlan")
                      : activeGoalMode
                        ? t("composer.placeholderGoal")
                      : isStreaming
                        ? t("composer.placeholderStreaming")
                        : t("composer.placeholder")
                  }
                  rows={1}
                  disabled={modelBusy || hasBlockingInteraction}
                />
                <div className="composer-toolbar">
                  <div className="composer-options">
                    <div
                      className={`composer-add-menu-wrap ${
                        composerAddOpen ? "open" : ""
                      }`}
                      ref={composerAddRef}
                    >
                      <button
                        className="toolbar-icon composer-add-trigger"
                        type="button"
                        title={t("composer.add")}
                        aria-label={t("composer.add")}
                        aria-expanded={composerAddOpen}
                        aria-controls="composer-add-menu"
                        onClick={toggleComposerAdd}
                        disabled={!selectedModel || modelBusy}
                      >
                        <Plus size={15} />
                      </button>
                      {composerAddOpen && (
                        <div
                          className="composer-add-menu"
                          id="composer-add-menu"
                          role="menu"
                          aria-label={t("composer.addMenu")}
                        >
                          <div className="composer-add-group">
                            <button
                              className="composer-add-item"
                              type="button"
                              role="menuitem"
                              disabled={
                                promptAttachments.length >=
                                MAX_PROMPT_ATTACHMENTS
                              }
                              onClick={() => {
                                setComposerAddOpen(false);
                                attachmentInputRef.current?.click();
                              }}
                            >
                              <Paperclip size={15} />
                              <span>
                                <strong>{t("composer.attachments")}</strong>
                                <small>{t("composer.attachmentsDesc")}</small>
                              </span>
                            </button>
                            <button
                              className={`composer-add-item ${
                                activePlan ? "selected" : ""
                              }`}
                              type="button"
                              role="menuitemcheckbox"
                              aria-checked={Boolean(activePlan)}
                              disabled={
                                !activeAgentScope || modeBusy || isStreaming
                              }
                              onClick={() => {
                                setComposerAddOpen(false);
                                void togglePlanMode();
                              }}
                            >
                              <ClipboardList size={15} />
                              <span>
                                <strong>{t("plan.label")}</strong>
                                <small>{t("plan.desc")}</small>
                              </span>
                              {activePlan && <Check size={14} />}
                            </button>
                            <button
                              className={`composer-add-item ${
                                activeGoal || activeGoalMode ? "selected" : ""
                              }`}
                              type="button"
                              role="menuitemcheckbox"
                              aria-checked={Boolean(
                                activeGoal || activeGoalMode,
                              )}
                              disabled={
                                !activeAgentScope ||
                                modeBusy ||
                                activeGoal?.status === "complete"
                              }
                              onClick={() => {
                                setComposerAddOpen(false);
                                if (activeGoal?.status === "active") {
                                  void controlActiveGoal("pause");
                                } else if (
                                  activeGoal?.status === "paused" ||
                                  activeGoal?.status === "blocked"
                                ) {
                                  void controlActiveGoal("resume");
                                } else {
                                  void toggleGoalMode();
                                }
                              }}
                            >
                              <Target size={15} />
                              <span>
                                <strong>{t("goal.label")}</strong>
                                <small>
                                  {activeGoal?.status === "active"
                                    ? t("goal.pauseDesc")
                                    : activeGoal?.status === "paused" ||
                                        activeGoal?.status === "blocked"
                                      ? t("goal.resumeDesc")
                                      : activeGoal?.objective ?? t("goal.desc")}
                                </small>
                              </span>
                              {(activeGoal || activeGoalMode) && (
                                <Check size={14} />
                              )}
                            </button>
                            <button
                              className={`composer-add-item ${
                                activeSwarmMode ? "selected" : ""
                              }`}
                              type="button"
                              role="menuitemcheckbox"
                              aria-checked={activeSwarmMode}
                              disabled={
                                !activeAgentScope || modeBusy || isStreaming
                              }
                              onClick={() => {
                                setComposerAddOpen(false);
                                void toggleSwarmMode();
                              }}
                            >
                              <RemixSparklingLineIcon size={15} />
                              <span>
                                <strong>{t("swarm.label")}</strong>
                                <small>
                                  {activeSwarmMode
                                    ? t("swarm.disableDesc")
                                    : t("swarm.desc")}
                                </small>
                              </span>
                              {activeSwarmMode && <Check size={14} />}
                            </button>
                          </div>

                          <div className="composer-add-divider" />
                          <div className="composer-add-heading">{t("skills.heading")}</div>
                          <div className="composer-skill-list">
                            {skillsBusy ? (
                              <div className="composer-add-empty">
                                <span className="spinner" />
                                {t("skills.loading")}
                              </div>
                            ) : skillsError ? (
                              <div className="composer-add-empty error">
                                {skillsError}
                                <button
                                  type="button"
                                  onClick={() => void loadAvailableSkills()}
                                >
                                  {t("common.retry")}
                                </button>
                              </div>
                            ) : availableSkills.length === 0 ? (
                              <div className="composer-add-empty">
                                {t("skills.empty")}
                              </div>
                            ) : (
                              sortSkillsForAddMenu(availableSkills).map((skill) => {
                                const selected = promptSkills.some(
                                  (item) => item.name === skill.name,
                                );
                                return (
                                  <button
                                    className={`composer-add-item skill ${
                                      selected ? "selected" : ""
                                    }`}
                                    type="button"
                                    role="menuitemcheckbox"
                                    aria-checked={selected}
                                    key={skill.name}
                                    onClick={() => selectPromptSkill(skill)}
                                  >
                                    <Package size={15} />
                                    <span>
                                      <strong>{skillDisplayName(skill.name)}</strong>
                                      <small>{skill.description}</small>
                                    </span>
                                    {selected && <Check size={14} />}
                                  </button>
                                );
                              })
                            )}
                          </div>
                        </div>
                      )}
                    </div>
                    <ToolbarSelect
                      className="model-select"
                      ariaLabel={t("model.select")}
                      icon={<Bot size={15} />}
                      value={selectedModel?.id ?? ""}
                      label={
                        modelsBusy
                          ? t("model.syncing")
                          : (selectedModel?.displayName ?? t("model.none"))
                      }
                      disabled={
                        modelsBusy ||
                        modelBusy ||
                        !activeAgentScope ||
                        !models.length
                      }
                      options={models.map((model) => ({
                        value: model.id,
                        label: model.displayName,
                        description: t("model.contextDesc", { size: formatContext(model.contextLength) }),
                      }))}
                      onChange={chooseModel}
                    />
                    {selectedModel?.supportsReasoning &&
                      supportedThinkingLevels.length > 0 && (
                        <ToolbarSelect
                          className="effort-select"
                          ariaLabel={t("thinking.select")}
                          icon={<BrainCircuit size={15} />}
                          value={effort}
                          label={t("thinking.label", { level: effort })}
                          disabled={modelBusy || !activeAgentScope}
                          options={supportedThinkingLevels.map((value) => ({
                            value,
                            label: t("thinking.label", { level: value }),
                            description: thinkingLevelDescription(value),
                          }))}
                          onChange={chooseEffort}
                        />
                      )}
                    <ToolbarSelect
                      className={`permission-select ${
                        permissionMode === "yolo"
                          ? "full-access"
                          : permissionMode === "auto"
                            ? "auto-access"
                            : ""
                      }`}
                      ariaLabel={t("permission.select")}
                      icon={<ShieldCheck size={15} />}
                      value={permissionMode}
                      label={
                        permissionMode === "yolo"
                          ? t("permission.yolo")
                          : permissionMode === "auto"
                            ? t("permission.auto")
                            : t("permission.manual")
                      }
                      disabled={modelBusy}
                      options={[
                        {
                          value: "manual",
                          label: t("permission.manual"),
                          description: t("permission.manualDesc"),
                        },
                        {
                          value: "auto",
                          label: t("permission.auto"),
                          description: t("permission.autoDesc"),
                        },
                        {
                          value: "yolo",
                          label: t("permission.yolo"),
                          description: t("permission.yoloDesc"),
                          danger: true,
                        },
                      ]}
                      onChange={(value) =>
                        choosePermissionMode(value as PermissionMode)
                      }
                    />
                    {selectedModel && (
                      <ContextUsageIndicator
                        usage={activeContextUsage}
                        agentUsage={activeAgentUsage}
                        models={models}
                        maxContextTokens={selectedModel.contextLength}
                      />
                    )}
                  </div>
                  <div className="send-zone">
                    <span>{isStreaming ? t("composer.enterQueue") : t("composer.enterSend")}</span>
                    <button
                      className="send-button"
                      type={showStopButton ? "button" : "submit"}
                      onClick={
                        showStopButton
                          ? () => void cancelActiveTurn()
                          : undefined
                      }
                      disabled={
                        showStopButton
                          ? !activeAgentScope
                          : hasBlockingInteraction ||
                            !composerHasContent ||
                            isHistoryLoading ||
                            modelBusy ||
                            !activeAgentScope ||
                            (promptAttachments.some(
                              (attachment) => attachment.kind === "image",
                            ) &&
                              !selectedModel?.supportsImage) ||
                            (promptAttachments.some(
                              (attachment) => attachment.kind === "video",
                            ) &&
                              !selectedModel?.supportsVideo)
                      }
                      title={showStopButton ? t("composer.stop") : isStreaming ? t("composer.queue") : t("composer.send")}
                    >
                      {showStopButton ? <X size={17} /> : <ArrowUp size={18} />}
                    </button>
                  </div>
                </div>
              </form>
              <p className="composer-caption">
                {t("composer.caption")}
              </p>
            </div>
  );
}
