import type { CompactionEvent, ProtocolMessage } from "../types";
import type { HistoryConversationTurn } from "./history";
import type { InFlightTurn } from "./liveTurns";

export interface LiveCompactionAnchor {
  turnKey: string;
  stepKey: string;
  afterBlockIndex: number;
  liveBlockId: string;
}

export type LiveCompactionEvent = CompactionEvent & {
  liveAnchor?: LiveCompactionAnchor;
};

export function liveTurnKey(turn: InFlightTurn): string {
  return turn.promptId ?? turn.userMessageId ?? turn.createdAt;
}

export function currentLiveCompactionAnchor(
  turn: InFlightTurn,
): LiveCompactionAnchor {
  const step = turn.steps.at(-1);
  const compactionCount = turn.steps.reduce(
    (count, item) =>
      count + item.blocks.filter((block) => block.kind === "compaction").length,
    0,
  );
  return {
    turnKey: liveTurnKey(turn),
    stepKey: step?.stepId ?? `step-${step?.step ?? 0}`,
    afterBlockIndex: (step?.blocks.length ?? 0) - 1,
    liveBlockId: `compaction-${compactionCount}`,
  };
}

export function messageOriginKind(message: ProtocolMessage): string | undefined {
  const origin = message.metadata?.origin;
  return origin && typeof origin === "object" && "kind" in origin
    ? String(origin.kind)
    : undefined;
}

export function groupHistoryMessages(
  messages: ProtocolMessage[],
): HistoryConversationTurn[] {
  const turns: HistoryConversationTurn[] = [];

  for (const message of messages) {
    if (messageOriginKind(message) === "compaction_summary") {
      const turn = turns.at(-1);
      if (turn) {
        turn.responses.push(message);
      } else {
        turns.push({ id: message.id, responses: [message] });
      }
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
    if (!turn) {
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

export function compactionSummaryForLiveTurn(
  messages: ProtocolMessage[],
  turn: InFlightTurn,
): ProtocolMessage | undefined {
  let boundaryIndex = -1;
  if (turn.userMessageId) {
    boundaryIndex = messages.findIndex(
      (message) => message.id === turn.userMessageId,
    );
  }
  if (boundaryIndex < 0 && turn.promptId) {
    boundaryIndex = messages.findIndex(
      (message) =>
        message.role === "user" && message.prompt_id === turn.promptId,
    );
  }
  if (boundaryIndex < 0 && turn.historyBoundaryId) {
    boundaryIndex = messages.findIndex(
      (message) => message.id === turn.historyBoundaryId,
    );
  }
  if (boundaryIndex < 0) return undefined;

  for (let index = messages.length - 1; index > boundaryIndex; index -= 1) {
    const message = messages[index];
    if (messageOriginKind(message) === "compaction_summary") return message;
  }
  return undefined;
}

export function updateLiveCompaction(
  turn: InFlightTurn,
  event: CompactionEvent,
): InFlightTurn {
  let latestCompaction:
    | { stepIndex: number; blockIndex: number; phase: CompactionEvent["phase"] }
    | undefined;
  let startedCompaction: { stepIndex: number; blockIndex: number } | undefined;
  for (let stepIndex = turn.steps.length - 1; stepIndex >= 0; stepIndex -= 1) {
    const blocks = turn.steps[stepIndex].blocks;
    for (let blockIndex = blocks.length - 1; blockIndex >= 0; blockIndex -= 1) {
      const block = blocks[blockIndex];
      if (block.kind !== "compaction") continue;
      latestCompaction ??= {
        stepIndex,
        blockIndex,
        phase: block.event.phase,
      };
      if (block.event.phase === "started") {
        startedCompaction = { stepIndex, blockIndex };
        break;
      }
    }
    if (startedCompaction) break;
  }

  const target =
    event.phase === "started"
      ? latestCompaction?.phase === "started"
        ? latestCompaction
        : undefined
      : startedCompaction ??
        (latestCompaction?.phase === event.phase
          ? latestCompaction
          : undefined);
  if (target) {
    const step = turn.steps[target.stepIndex];
    const block = step.blocks[target.blockIndex];
    if (block.kind !== "compaction") return turn;
    const blocks = [...step.blocks];
    blocks[target.blockIndex] = { ...block, event };
    const steps = [...turn.steps];
    steps[target.stepIndex] = { ...step, blocks };
    return { ...turn, steps };
  }

  const compactionCount = turn.steps.reduce(
    (count, step) =>
      count + step.blocks.filter((block) => block.kind === "compaction").length,
    0,
  );
  const steps =
    turn.steps.length > 0
      ? [...turn.steps]
      : [{ step: 0, status: "running" as const, blocks: [] }];
  const stepIndex = steps.length - 1;
  const step = steps[stepIndex];
  steps[stepIndex] = {
    ...step,
    blocks: [
      ...step.blocks,
      {
        kind: "compaction",
        id: `compaction-${compactionCount}`,
        event,
      },
    ],
  };
  return { ...turn, steps };
}
