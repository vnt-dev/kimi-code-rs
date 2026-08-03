import { t } from "../i18n";
import { parseSkillPromptDisplay } from "../prompt/skills";
import type { MessageContent, ProtocolMessage } from "../types";
import type { InFlightTurn } from "./liveTurns";
import { displayMessageText, messageText } from "./messages";

export type RenderMessage = ProtocolMessage & {
  status?: "streaming" | "done" | "error";
};

export type ToolResultContent = Extract<
  MessageContent,
  { type: "tool_result" }
>;

export interface HistoryToolPresentation {
  messages: ProtocolMessage[];
  results: Map<string, ToolResultContent>;
}

export interface HistoryConversationTurn {
  id: string;
  user?: RenderMessage;
  responses: RenderMessage[];
}

export function messageOriginKind(message: ProtocolMessage): string | undefined {
  const origin = message.metadata?.origin;
  return origin && typeof origin === "object" && "kind" in origin
    ? String(origin.kind)
    : undefined;
}

export function isDirectUserMessage(message: ProtocolMessage): boolean {
  const origin = messageOriginKind(message);
  return origin === undefined || origin === "user";
}

export function isVisibleHistoryMessage(message: ProtocolMessage): boolean {
  return !["injection", "system_trigger", "task", "cron"].includes(
    messageOriginKind(message) ?? "",
  );
}

export function historyBeforeInFlightTurn(
  items: ProtocolMessage[],
  turn: InFlightTurn,
): ProtocolMessage[] {
  if (turn.userMessageId) {
    const userMessage = items.findIndex(
      (message) => message.id === turn.userMessageId,
    );
    if (userMessage >= 0) return items.slice(0, userMessage);
  }
  if (turn.historyBoundaryId) {
    const boundary = items.findIndex(
      (message) => message.id === turn.historyBoundaryId,
    );
    if (boundary >= 0) return items.slice(0, boundary + 1);
  }

  const prompt = parseSkillPromptDisplay(turn.prompt).text;
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const message = items[index];
    if (message.role === "user" && displayMessageText(message) === prompt) {
      return items.slice(0, index);
    }
  }
  return items;
}

export function completedTurnMessageId(
  items: ProtocolMessage[],
  turn: InFlightTurn,
): string | undefined {
  let startIndex = 0;
  if (turn.userMessageId) {
    const userMessage = items.findIndex(
      (message) => message.id === turn.userMessageId,
    );
    if (userMessage >= 0) startIndex = userMessage + 1;
  } else if (turn.historyBoundaryId) {
    const boundary = items.findIndex(
      (message) => message.id === turn.historyBoundaryId,
    );
    if (boundary >= 0) startIndex = boundary + 1;
  } else {
    for (let index = items.length - 1; index >= 0; index -= 1) {
      const message = items[index];
      if (
        message.role === "user" &&
        displayMessageText(message) ===
          parseSkillPromptDisplay(turn.prompt).text
      ) {
        startIndex = index + 1;
        break;
      }
    }
  }

  for (let index = items.length - 1; index >= startIndex; index -= 1) {
    if (items[index].role === "assistant") return items[index].id;
  }
  return undefined;
}

export function formatElapsedDuration(durationMs: number): string {
  const totalSeconds = Math.max(0, durationMs) / 1000;
  if (totalSeconds < 10)
    return t("duration.seconds", { value: totalSeconds.toFixed(1) });
  const roundedSeconds = Math.round(totalSeconds);
  if (roundedSeconds < 60)
    return t("duration.seconds", { value: roundedSeconds });
  const minutes = Math.floor(roundedSeconds / 60);
  const seconds = roundedSeconds % 60;
  return seconds > 0
    ? t("duration.minutesSeconds", { minutes, seconds })
    : t("duration.minutes", { value: minutes });
}

export function mergeHistoryToolResults(
  messages: ProtocolMessage[],
): HistoryToolPresentation {
  const results = new Map<string, ToolResultContent>();

  for (const message of messages) {
    for (const part of message.content) {
      if (part.type === "tool_result") results.set(part.tool_call_id, part);
    }
  }

  const mergedMessages = messages.flatMap((message) => {
    const content = message.content.filter((part) => part.type !== "tool_result");
    if (content.length === 0) return [];
    return content.length === message.content.length
      ? [message]
      : [{ ...message, content }];
  });

  return { messages: mergedMessages, results };
}

export function groupHistoryMessages(
  messages: ProtocolMessage[],
): HistoryConversationTurn[] {
  const turns: HistoryConversationTurn[] = [];

  for (const message of messages) {
    if (messageOriginKind(message) === "compaction_summary") {
      turns.push({
        id: message.id,
        responses: [message],
      });
      continue;
    }
    if (message.role === "user") {
      turns.push({
        id: message.prompt_id ?? message.id,
        user: message,
        responses: [],
      });
      continue;
    }

    let turn = turns.at(-1);
    if (
      !turn ||
      turn.responses.some(
        (response) =>
          messageOriginKind(response) === "compaction_summary",
      )
    ) {
      turn = {
        id: message.prompt_id ?? message.id,
        responses: [],
      };
      turns.push(turn);
    }
    turn.responses.push(message);
  }

  return turns;
}

export function finalResponseMessage(
  messages: RenderMessage[],
): RenderMessage | undefined {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (
      message.role === "assistant" &&
      message.status !== "streaming" &&
      messageOriginKind(message) !== "compaction_summary" &&
      messageText(message).trim().length > 0
    ) {
      return message;
    }
  }
  return undefined;
}
