import type {
  AgentInteraction,
  ApprovalPayload,
  CommandDisplay,
  PlanReviewDisplay,
  QuestionPayload,
} from "./types";

export type ConversationStatus = "attention" | "running" | "completed";
export type UserAttentionKind = "question" | "approval" | "planReview";

export interface UserAttention {
  kind: UserAttentionKind;
  content?: string;
}

export function userAttentionForInteraction(
  interaction: AgentInteraction,
): UserAttention | undefined {
  if (interaction.kind === "question") {
    const payload = interaction.payload as QuestionPayload;
    return {
      kind: "question",
      content: payload.questions
        ?.map((question) => question.question.trim())
        .filter(Boolean)
        .join("\n"),
    };
  }
  if (interaction.kind !== "approval") return undefined;

  const payload = interaction.payload as ApprovalPayload;
  if (payload.display?.kind === "plan_review") {
    const plan = (payload.display as Partial<PlanReviewDisplay>).plan;
    return {
      kind: "planReview",
      content: typeof plan === "string" ? plan : undefined,
    };
  }
  const command = payload.display as Partial<CommandDisplay>;
  return {
    kind: "approval",
    content:
      payload.action ||
      (command.kind === "command"
        ? (typeof command.description === "string" && command.description) ||
          (typeof command.command === "string" ? command.command : undefined)
        : undefined) ||
      payload.toolName,
  };
}

export function hasUserAttention(
  interactions: AgentInteraction[] | undefined,
): boolean {
  return interactions?.some(
    (interaction) => userAttentionForInteraction(interaction) !== undefined,
  ) ?? false;
}

export function conversationStatus({
  interactions,
  running,
  completedUnread,
}: {
  interactions?: AgentInteraction[];
  running: boolean;
  completedUnread: boolean;
}): ConversationStatus | undefined {
  if (hasUserAttention(interactions)) return "attention";
  if (running) return "running";
  if (completedUnread) return "completed";
  return undefined;
}
