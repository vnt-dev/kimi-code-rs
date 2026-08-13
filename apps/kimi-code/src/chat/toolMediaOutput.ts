export type ToolMediaKind = "image" | "audio" | "video";

export interface ToolMediaOutputItem {
  kind: ToolMediaKind;
  url: string;
  path?: string;
}

export interface ParsedToolMediaOutput {
  items: ToolMediaOutputItem[];
  remaining: unknown[];
}

interface MediaContainer {
  kind: ToolMediaKind;
  path?: string;
}

const MEDIA_PARTS = {
  image_url: { kind: "image", property: "imageUrl" },
  audio_url: { kind: "audio", property: "audioUrl" },
  video_url: { kind: "video", property: "videoUrl" },
} as const;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function mediaContainer(text: string): MediaContainer | null | undefined {
  const trimmed = text.trim();
  const closing = trimmed.match(/^<\/(image|audio|video)>$/i);
  if (closing) return null;

  const opening = trimmed.match(
    /^<(image|audio|video)\s+path\s*=\s*(["'])(.*?)\2\s*>$/i,
  );
  if (!opening) return undefined;
  return {
    kind: opening[1].toLowerCase() as ToolMediaKind,
    path: opening[3].trim() || undefined,
  };
}

function mediaPart(
  value: unknown,
  container?: MediaContainer,
): ToolMediaOutputItem | undefined {
  if (!isRecord(value) || typeof value.type !== "string") return undefined;
  const definition = MEDIA_PARTS[value.type as keyof typeof MEDIA_PARTS];
  if (!definition) return undefined;

  const media = value[definition.property];
  if (!isRecord(media) || typeof media.url !== "string" || !media.url.trim()) {
    return undefined;
  }

  return {
    kind: definition.kind,
    url: media.url,
    path:
      container?.kind === definition.kind
        ? container.path
        : typeof media.id === "string" && media.id.trim()
          ? media.id
          : undefined,
  };
}

/**
 * Parses ReadMediaFile's tagged structured output while retaining any content
 * that is not part of a media wrapper for a readable fallback.
 */
export function parseToolMediaOutput(
  output: unknown,
): ParsedToolMediaOutput | undefined {
  if (!Array.isArray(output)) return undefined;

  const items: ToolMediaOutputItem[] = [];
  const remaining: unknown[] = [];
  let container: MediaContainer | undefined;

  for (const part of output) {
    if (isRecord(part) && part.type === "text" && typeof part.text === "string") {
      const tag = mediaContainer(part.text);
      if (tag === null) {
        container = undefined;
        continue;
      }
      if (tag !== undefined) {
        container = tag;
        continue;
      }
    }

    const media = mediaPart(part, container);
    if (media) {
      items.push(media);
    } else {
      remaining.push(part);
    }
  }

  return items.length > 0 ? { items, remaining } : undefined;
}
