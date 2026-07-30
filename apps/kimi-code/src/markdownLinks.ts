const SUPPORTED_MARKDOWN_LINK_PROTOCOLS = new Set([
  "http:",
  "https:",
  "mailto:",
]);

export function resolveMarkdownExternalUrl(
  href: string | undefined,
): string | undefined {
  const trimmed = href?.trim();
  if (!trimmed) return undefined;

  const candidate = trimmed.startsWith("//")
    ? `https:${trimmed}`
    : /^www\./i.test(trimmed)
      ? `https://${trimmed}`
      : trimmed;

  try {
    const url = new URL(candidate);
    return SUPPORTED_MARKDOWN_LINK_PROTOCOLS.has(url.protocol)
      ? url.toString()
      : undefined;
  } catch {
    return undefined;
  }
}
