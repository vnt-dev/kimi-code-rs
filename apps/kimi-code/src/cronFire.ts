export interface CronFireMessage {
  cron: string;
  recurring: boolean;
  coalescedCount: number;
  stale: boolean;
  prompt: string;
}

function decodeXmlAttribute(value: string): string {
  return value.replace(
    /&(amp|quot|apos|lt|gt);/g,
    (_, entity: string) =>
      ({ amp: "&", quot: '"', apos: "'", lt: "<", gt: ">" })[entity] ?? _,
  );
}

export function parseCronFireMessage(text: string): CronFireMessage | undefined {
  const envelope = text.trim();
  const match = /^<cron-fire\b([^>]*)>\s*<prompt>\r?\n?([\s\S]*)\r?\n?<\/prompt>\s*<\/cron-fire>$/.exec(
    envelope,
  );
  if (!match) return undefined;

  const attributes = new Map<string, string>();
  for (const attribute of match[1].matchAll(/([A-Za-z][\w-]*)="([^"]*)"/g)) {
    attributes.set(attribute[1], decodeXmlAttribute(attribute[2]));
  }

  const cron = attributes.get("cron")?.trim();
  const recurring = attributes.get("recurring");
  const stale = attributes.get("stale");
  const coalescedCount = Number(attributes.get("coalescedCount"));
  const prompt = match[2].trim();
  if (
    !cron ||
    !prompt ||
    (recurring !== "true" && recurring !== "false") ||
    (stale !== "true" && stale !== "false") ||
    !Number.isSafeInteger(coalescedCount) ||
    coalescedCount < 1
  ) {
    return undefined;
  }

  return {
    cron,
    recurring: recurring === "true",
    coalescedCount,
    stale: stale === "true",
    prompt,
  };
}
