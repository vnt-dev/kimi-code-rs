export function normalizeRetryAttempt(failedAttempt: number): number {
  if (!Number.isFinite(failedAttempt)) return 1;
  return Math.max(1, Math.trunc(failedAttempt));
}

export function clearRetryStatus<T extends { retry?: unknown }>(value: T): T {
  if (value.retry === undefined) return value;
  return { ...value, retry: undefined };
}

export function isVisibleRetryStep(
  step: {
    step: number;
    stepId?: string;
    blocks: readonly unknown[];
    interruption?: string;
  },
  steeredPrompts: readonly { anchorStepKey?: string }[],
): boolean {
  if (step.blocks.length > 0 || Boolean(step.interruption)) return true;
  const stepKey = step.stepId ?? `step-${step.step}`;
  return steeredPrompts.some((item) => item.anchorStepKey === stepKey);
}
