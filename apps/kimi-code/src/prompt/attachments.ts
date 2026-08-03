import { t } from "../i18n";
import { uploadFileTransport } from "../transport";
import type { AgentPromptPart } from "../types";
import type { PromptAttachment, PromptAttachmentKind } from "../chat/liveTurns";

export const MAX_PROMPT_ATTACHMENTS = 8;
const MAX_PROMPT_ATTACHMENT_BYTES = 20 * 1024 * 1024;
const MAX_PROMPT_IMAGE_DIMENSION = 2048;
const IMAGE_COMPRESSION_THRESHOLD = 4 * 1024 * 1024;
const PROMPT_IMAGE_TYPES = new Set([
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
]);
const PROMPT_AUDIO_TYPES = new Set([
  "audio/mpeg",
  "audio/mp3",
  "audio/wav",
  "audio/x-wav",
  "audio/ogg",
  "audio/webm",
  "audio/mp4",
]);
const PROMPT_VIDEO_TYPES = new Set([
  "video/mp4",
  "video/mpeg",
  "video/quicktime",
  "video/webm",
  "video/x-matroska",
  "video/x-msvideo",
  "video/3gpp",
]);

interface UploadedFileMeta {
  id: string;
  name: string;
  media_type: string;
  size: number;
}

export function promptAttachmentKind(
  mimeType: string,
): PromptAttachmentKind {
  if (PROMPT_IMAGE_TYPES.has(mimeType)) return "image";
  if (PROMPT_AUDIO_TYPES.has(mimeType)) return "audio";
  if (PROMPT_VIDEO_TYPES.has(mimeType)) return "video";
  return "file";
}

function readFileAsDataUrl(file: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () =>
      typeof reader.result === "string"
        ? resolve(reader.result)
        : reject(new Error(t("error.readMedia")));
    reader.onerror = () =>
      reject(reader.error ?? new Error(t("error.readMedia")));
    reader.readAsDataURL(file);
  });
}

function canvasToBlob(
  canvas: HTMLCanvasElement,
  type: string,
  quality?: number,
): Promise<Blob> {
  return new Promise((resolve, reject) => {
    canvas.toBlob(
      (blob) =>
        blob ? resolve(blob) : reject(new Error(t("error.processImage"))),
      type,
      quality,
    );
  });
}

export async function preparePromptAttachment(file: File): Promise<PromptAttachment> {
  const kind = promptAttachmentKind(file.type);
  if (file.size > MAX_PROMPT_ATTACHMENT_BYTES) {
    throw new Error(t("error.fileTooLarge", { name: file.name }));
  }

  if (kind === "file") {
    const uploaded = (await uploadFileTransport(
      file,
      file.name || "attachment",
    )) as UploadedFileMeta;
    return {
      id: uploaded.id,
      fileId: uploaded.id,
      name: uploaded.name,
      mediaType: uploaded.media_type,
      size: uploaded.size,
      kind,
    };
  }

  let payload: Blob = file;
  if (kind === "image" && file.type !== "image/gif") {
    const bitmap = await createImageBitmap(file);
    try {
      const scale = Math.min(
        1,
        MAX_PROMPT_IMAGE_DIMENSION / Math.max(bitmap.width, bitmap.height),
      );
      if (scale < 1 || file.size > IMAGE_COMPRESSION_THRESHOLD) {
        const canvas = document.createElement("canvas");
        canvas.width = Math.max(1, Math.round(bitmap.width * scale));
        canvas.height = Math.max(1, Math.round(bitmap.height * scale));
        const context = canvas.getContext("2d", { alpha: false });
        if (!context) throw new Error(t("error.processImage"));
        context.fillStyle = "#ffffff";
        context.fillRect(0, 0, canvas.width, canvas.height);
        context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
        payload = await canvasToBlob(canvas, "image/jpeg", 0.86);
      }
    } finally {
      bitmap.close();
    }
  }

  return {
    id:
      typeof crypto.randomUUID === "function"
        ? crypto.randomUUID()
        : `${Date.now()}-${Math.random().toString(36).slice(2)}`,
    name: file.name || `clipboard-${kind}`,
    dataUrl: await readFileAsDataUrl(payload),
    mediaType: payload.type || file.type || "application/octet-stream",
    size: payload.size,
    kind,
  };
}

export function buildAgentPromptInput(
  text: string,
  attachments: readonly PromptAttachment[],
): AgentPromptPart[] {
  return [
    ...(text ? [{ type: "text" as const, text }] : []),
    ...attachments.map((attachment): AgentPromptPart => {
      switch (attachment.kind) {
        case "image":
          return {
            type: "image_url",
            imageUrl: { url: attachment.dataUrl!, id: attachment.id },
          };
        case "audio":
          return {
            type: "audio_url",
            audioUrl: { url: attachment.dataUrl!, id: attachment.id },
          };
        case "video":
          return {
            type: "video_url",
            videoUrl: { url: attachment.dataUrl!, id: attachment.id },
          };
        case "file":
          return {
            type: "file",
            file_id: attachment.fileId!,
            name: attachment.name,
            media_type: attachment.mediaType,
            size: attachment.size,
          };
      }
    }),
  ];
}
