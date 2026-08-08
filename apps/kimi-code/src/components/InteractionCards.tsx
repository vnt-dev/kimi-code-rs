import { useState } from "react";
import {
  Check,
  ClipboardList,
  MessageSquareText,
  RefreshCw,
  ShieldAlert,
  TerminalSquare,
} from "lucide-react";

import { t } from "../i18n";
import {
  isRetryConfirmationPayload,
  retryConfirmationResponse,
} from "../retryConfirmation";
import type {
  AgentInteraction,
  ApprovalPayload,
  PlanReviewDisplay,
  QuestionPayload,
  QuestionResponse,
} from "../types";
import { MarkdownMessage } from "./chat/MarkdownMessage";

export function isPlanReviewInteraction(interaction: AgentInteraction): boolean {
  const payload = interaction.payload as Partial<ApprovalPayload>;
  return payload.display?.kind === "plan_review";
}

export function isRetryConfirmationInteraction(
  interaction: AgentInteraction,
): boolean {
  return isRetryConfirmationPayload(interaction.payload);
}

export function RetryConfirmationCard({
  busy,
  onCancel,
  onContinue,
}: {
  busy: boolean;
  onCancel: () => void;
  onContinue: (response: QuestionResponse) => void;
}) {
  return (
    <section
      className="interaction-card retry-confirmation-card"
      aria-live="polite"
    >
      <div className="interaction-card-heading">
        <span className="interaction-card-icon">
          <RefreshCw size={18} />
        </span>
        <div>
          <strong>{t("retryConfirmation.message")}</strong>
        </div>
      </div>
      <div className="interaction-card-actions">
        <button
          type="button"
          className="interaction-secondary"
          disabled={busy}
          onClick={onCancel}
        >
          {t("common.cancel")}
        </button>
        <button
          type="button"
          className="interaction-primary"
          disabled={busy}
          onClick={() => onContinue(retryConfirmationResponse())}
        >
          {busy ? <span className="spinner light" /> : <RefreshCw size={14} />}
          {t("retryConfirmation.continue")}
        </button>
      </div>
    </section>
  );
}

export function QuestionCard({
  interaction,
  busy,
  onRespond,
}: {
  interaction: AgentInteraction;
  busy: boolean;
  onRespond: (response: QuestionResponse | null) => void;
}) {
  const payload = interaction.payload as QuestionPayload;
  const questions = Array.isArray(payload.questions) ? payload.questions : [];
  const [selections, setSelections] = useState<Record<number, string[]>>({});
  const [otherAnswers, setOtherAnswers] = useState<Record<number, string>>({});

  const toggleOption = (
    questionIndex: number,
    label: string,
    multiSelect: boolean,
  ): void => {
    setSelections((current) => {
      const selected = current[questionIndex] ?? [];
      const next = multiSelect
        ? selected.includes(label)
          ? selected.filter((value) => value !== label)
          : [...selected, label]
        : [label];
      return { ...current, [questionIndex]: next };
    });
    if (!multiSelect) {
      setOtherAnswers((current) => ({ ...current, [questionIndex]: "" }));
    }
  };

  const updateOtherAnswer = (
    questionIndex: number,
    value: string,
    multiSelect: boolean,
  ): void => {
    setOtherAnswers((current) => ({ ...current, [questionIndex]: value }));
    if (!multiSelect && value.trim()) {
      setSelections((current) => ({ ...current, [questionIndex]: [] }));
    }
  };

  const answers = questions.map((_question, questionIndex) => {
    const selected = selections[questionIndex] ?? [];
    const other = otherAnswers[questionIndex]?.trim();
    return other ? [...selected, other] : selected;
  });
  const canSubmit =
    questions.length > 0 && answers.every((answer) => answer.length > 0);

  const submit = (): void => {
    if (!canSubmit || busy) return;
    const responseAnswers: Record<string, string> = {};
    questions.forEach((question, questionIndex) => {
      responseAnswers[question.question] = answers[questionIndex]?.join(", ") ?? "";
    });
    onRespond({ answers: responseAnswers, method: "enter" });
  };

  return (
    <section className="interaction-card question-card" aria-live="polite">
      <div className="interaction-card-heading">
        <span className="interaction-card-icon">
          <MessageSquareText size={18} />
        </span>
        <div>
          <small>{t("question.subtitle")}</small>
          <strong>{t("question.title")}</strong>
        </div>
      </div>
      <div className="question-list">
        {questions.map((question, questionIndex) => {
          const selected = selections[questionIndex] ?? [];
          const multiSelect = question.multiSelect === true;
          return (
            <fieldset className="question-item" key={`${question.question}-${questionIndex}`}>
              <legend>
                {question.header && <span>{question.header}</span>}
                <strong>{question.question}</strong>
                {question.body && <small>{question.body}</small>}
              </legend>
              <div className="question-options">
                {question.options.map((option) => {
                  const checked = selected.includes(option.label);
                  return (
                    <button
                      type="button"
                      className={checked ? "selected" : ""}
                      key={option.label}
                      disabled={busy}
                      aria-pressed={checked}
                      onClick={() =>
                        toggleOption(questionIndex, option.label, multiSelect)
                      }
                    >
                      <span className={multiSelect ? "option-check" : "option-radio"}>
                        {checked && <Check size={12} />}
                      </span>
                      <span>
                        <strong>{option.label}</strong>
                        {option.description && <small>{option.description}</small>}
                      </span>
                    </button>
                  );
                })}
              </div>
              <label className="question-other">
                <span>{question.otherLabel || t("question.other")}</span>
                <input
                  value={otherAnswers[questionIndex] ?? ""}
                  disabled={busy}
                  placeholder={question.otherDescription || t("question.otherPlaceholder")}
                  onChange={(event) =>
                    updateOtherAnswer(questionIndex, event.target.value, multiSelect)
                  }
                />
              </label>
            </fieldset>
          );
        })}
      </div>
      <div className="interaction-card-actions">
        <button
          type="button"
          className="interaction-secondary"
          disabled={busy}
          onClick={() => onRespond(null)}
        >
          {t("question.skip")}
        </button>
        <button
          type="button"
          className="interaction-primary"
          disabled={busy || !canSubmit}
          onClick={submit}
        >
          {busy ? <span className="spinner light" /> : <Check size={14} />}
          {t("question.submit")}
        </button>
      </div>
    </section>
  );
}
export function PlanReviewCard({
  interaction,
  busy,
  onRespond,
}: {
  interaction: AgentInteraction;
  busy: boolean;
  onRespond: (response: Record<string, unknown>) => void;
}) {
  const payload = interaction.payload as ApprovalPayload;
  const display = payload.display as PlanReviewDisplay;
  const [feedback, setFeedback] = useState("");
  const options =
    display.options && display.options.length >= 2 ? display.options : [];
  const [selectedLabel, setSelectedLabel] = useState<string | undefined>(
    () => options[0]?.label,
  );
  const trimmedFeedback = feedback.trim();
  const needsRevision = trimmedFeedback.length > 0;
  const canExecute = options.length === 0 || selectedLabel !== undefined;

  const submitReview = (): void => {
    if (needsRevision) {
      onRespond({
        decision: "rejected",
        selectedLabel: "Revise",
        feedback: trimmedFeedback,
      });
      return;
    }
    onRespond({
      decision: "approved",
      selectedLabel: selectedLabel ?? "Approve",
    });
  };

  return (
    <section className="interaction-card plan-review-card" aria-live="polite">
      <div className="interaction-card-heading">
        <span className="interaction-card-icon">
          <ClipboardList size={18} />
        </span>
        <div>
          <small>{t("plan.completed")}</small>
          <strong>{t("plan.reviewTitle")}</strong>
        </div>
      </div>
      <div className="plan-review-content">
        <MarkdownMessage content={display.plan} />
      </div>
      {display.path && <code className="plan-review-path">{display.path}</code>}
      {options.length > 0 && (
        <div className="plan-review-options">
          <span>{t("plan.chooseOption")}</span>
          <div className="plan-review-option-list" role="radiogroup">
            {options.map((option) => (
              <label
                className={`plan-review-option ${
                  selectedLabel === option.label ? "selected" : ""
                } ${busy ? "disabled" : ""}`}
                key={option.label}
              >
                <input
                  type="radio"
                  name={`plan-review-${interaction.id}`}
                  value={option.label}
                  checked={selectedLabel === option.label}
                  disabled={busy}
                  onChange={() => setSelectedLabel(option.label)}
                />
                <span className="plan-review-option-copy">
                  <strong>{option.label}</strong>
                  {option.description && <small>{option.description}</small>}
                </span>
              </label>
            ))}
          </div>
        </div>
      )}
      <label className="plan-review-feedback">
        <span>{t("plan.feedbackLabel")}</span>
        <textarea
          rows={2}
          value={feedback}
          disabled={busy}
          placeholder={t("plan.feedbackPlaceholder")}
          onChange={(event) => setFeedback(event.target.value)}
        />
      </label>
      <div className="interaction-card-actions plan-review-actions">
        <button
          type="button"
          className="interaction-danger"
          disabled={busy}
          onClick={() =>
            onRespond({ decision: "rejected", selectedLabel: "Reject" })
          }
        >
          {t("common.reject")}
        </button>
        <button
          type="button"
          className={
            needsRevision ? "interaction-secondary" : "interaction-primary"
          }
          disabled={busy || (!needsRevision && !canExecute)}
          onClick={submitReview}
        >
          {busy ? (
            <span className="spinner light" />
          ) : needsRevision ? (
            <RefreshCw size={14} />
          ) : (
            <Check size={14} />
          )}
          {needsRevision ? t("plan.revise") : t("plan.execute")}
        </button>
      </div>
    </section>
  );
}

export function ApprovalCard({
  interaction,
  busy,
  onReject,
  onApprove,
  onApproveSession,
}: {
  interaction: AgentInteraction;
  busy: boolean;
  onReject: () => void;
  onApprove: () => void;
  onApproveSession: () => void;
}) {
  const payload = interaction.payload as ApprovalPayload;
  const display = payload.display;
  const isCommand = display?.kind === "command" && "command" in display;
  const command = isCommand ? String(display.command) : undefined;
  const cwd = isCommand && display.cwd ? String(display.cwd) : undefined;
  const detail =
    !isCommand && display
      ? ("path" in display && display.path) ||
        ("summary" in display && display.summary) ||
        payload.action
      : undefined;

  return (
    <section className="approval-card" aria-live="polite">
      <div className="approval-icon">
        <ShieldAlert size={19} />
      </div>
      <div className="approval-content">
        <div className="approval-heading">
          <div>
            <span>{t("approval.title")}</span>
            <strong>{payload.action || t("approval.toolRequest", { tool: payload.toolName })}</strong>
          </div>
          <span className="approval-tool">{payload.toolName}</span>
        </div>
        {command ? (
          <div className="approval-command">
            <div>
              <TerminalSquare size={13} />
              <span>{cwd || t("approval.currentDir")}</span>
            </div>
            <code>{command}</code>
          </div>
        ) : (
          <div className="approval-detail">{String(detail || t("approval.needsConfirm"))}</div>
        )}
        <div className="approval-footer">
          <p>{t("approval.warning")}</p>
          <div className="approval-actions">
            <button type="button" className="approval-reject" onClick={onReject} disabled={busy}>
              {t("common.reject")}
            </button>
            <button type="button" className="approval-session" onClick={onApproveSession} disabled={busy}>
              {t("approval.allowSession")}
            </button>
            <button type="button" className="approval-once" onClick={onApprove} disabled={busy}>
              {busy ? <span className="spinner light" /> : <Check size={14} />}
              {t("approval.allowOnce")}
            </button>
          </div>
        </div>
      </div>
    </section>
  );
}
