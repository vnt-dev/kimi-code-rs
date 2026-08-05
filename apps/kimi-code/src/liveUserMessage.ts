import type { LiveUserMessage, MessageContent } from "./types";
import {
  pluginCommandFromOrigin,
  pluginCommandText,
  type PluginCommandDisplay,
} from "./pluginCommandMessage.ts";

export type LiveMessageAttachmentKind = "image" | "audio" | "video" | "file";

export interface LiveMessageAttachment {
  id: string;
  name: string;
  dataUrl?: string;
  kind: LiveMessageAttachmentKind;
  fileId?: string;
  mediaType: string;
  size: number;
}

export interface ProjectedLiveUserMessage {
  text: string;
  attachments: LiveMessageAttachment[];
  pluginCommand?: PluginCommandDisplay;
  pluginCommandContent?: string;
}

function mediaSourceDataUrl(
  source: Extract<
    MessageContent,
    { type: "image" | "audio" | "video" }
  >["source"],
): string | undefined {
  if (source.kind === "url") return source.url;
  if (source.kind === "base64") {
    return `data:${source.media_type};base64,${source.data}`;
  }
  return undefined;
}

function projectAttachment(
  part: MessageContent,
  index: number,
  userMessageId: string,
): LiveMessageAttachment | undefined {
  if (part.type === "file") {
    return {
      id: part.file_id,
      fileId: part.file_id,
      name: part.name,
      kind: "file",
      mediaType: part.media_type,
      size: part.size,
    };
  }
  if (
    part.type !== "image" &&
    part.type !== "audio" &&
    part.type !== "video"
  ) {
    return undefined;
  }
  const media = part as Extract<
    MessageContent,
    { type: "image" | "audio" | "video" }
  >;
  const fileId = media.source.kind === "file" ? media.source.file_id : undefined;
  return {
    id: fileId ?? `${userMessageId}-${index}`,
    fileId,
    name: `${media.type}-${index + 1}`,
    dataUrl: mediaSourceDataUrl(media.source),
    kind: media.type,
    mediaType:
      media.source.kind === "base64"
        ? media.source.media_type
        : `${media.type}/*`,
    size: 0,
  };
}

export function projectLiveUserMessage(
  message: LiveUserMessage,
): ProjectedLiveUserMessage {
  const pluginCommand = pluginCommandFromOrigin(message.origin);
  const content = message.content
    .filter(
      (part): part is Extract<MessageContent, { type: "text" }> =>
        part.type === "text",
    )
    .map((part) => part.text)
    .join("");
  return {
    text: pluginCommand
      ? pluginCommandText(pluginCommand)
      : content,
    attachments: message.content.flatMap((part, index) => {
      const attachment = projectAttachment(part, index, message.userMessageId);
      return attachment ? [attachment] : [];
    }),
    pluginCommand,
    pluginCommandContent: pluginCommand ? content : undefined,
  };
}

export function isSameLiveUserMessage(
  current: { promptId?: string; userMessageId?: string },
  incoming: LiveUserMessage,
): boolean {
  return (
    current.promptId === incoming.promptId ||
    current.userMessageId === incoming.userMessageId
  );
}
