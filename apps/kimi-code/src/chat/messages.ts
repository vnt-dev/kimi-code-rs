import { parseSkillPromptDisplay } from "../prompt/skills";
import type { MessageContent, ProtocolMessage } from "../types";
import {
  pluginCommandFromOrigin,
  pluginCommandText,
  type PluginCommandDisplay,
} from "../pluginCommandMessage";

export type { PluginCommandDisplay } from "../pluginCommandMessage";

export function messagePluginCommand(
  message: ProtocolMessage,
): PluginCommandDisplay | undefined {
  return pluginCommandFromOrigin(message.metadata?.origin);
}

export function messageText(message: ProtocolMessage): string {
  return message.content
    .filter(
      (part): part is Extract<MessageContent, { type: "text" }> =>
        part.type === "text" && embeddedMediaContent(part.text) === undefined,
    )
    .map((part) => part.text)
    .join("");
}

export function displayMessageText(message: ProtocolMessage): string {
  const pluginCommand = messagePluginCommand(message);
  if (pluginCommand) return pluginCommandText(pluginCommand);
  return parseSkillPromptDisplay(messageText(message)).text;
}

export function embeddedMediaContent(text: string): MessageContent | undefined {
  for (const type of ["audio", "video"] as const) {
    const prefix = `[${type}:`;
    if (text.startsWith(prefix) && text.endsWith("]")) {
      const url = text.slice(prefix.length, -1);
      if (url) return { type, source: { kind: "url", url } };
    }
  }
  return undefined;
}
export function messageStructuredContent(message: ProtocolMessage): MessageContent[] {
  return message.content.flatMap((part) => {
    if (part.type === "thinking") return [];
    if (part.type !== "text") return [part];
    const media = embeddedMediaContent(part.text);
    return media ? [media] : [];
  });
}

export function messageThinking(message: ProtocolMessage): string {
  return message.content
    .filter(
      (part): part is Extract<MessageContent, { type: "thinking" }> =>
        part.type === "thinking",
    )
    .map((part) => part.thinking)
    .join("");
}

export function structuredValue(value: unknown): string {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

export function mediaSourceUrl(
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
