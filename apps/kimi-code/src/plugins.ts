import type { PluginCommandDef } from "./agentRpc";

export type PluginSource = "local-path" | "zip-url" | "github";
export type PluginState = "ok" | "error";
export type PluginMarketplaceTier = "official" | "curated";
export type PluginTab = "installed" | "official" | "third-party" | "custom";
export type PluginInstallPhase =
  | "resolving"
  | "downloading"
  | "extracting"
  | "installing"
  | "complete";

export interface PluginInstallProgressEvent {
  operationId: string;
  phase: PluginInstallPhase;
  downloadedBytes: number;
  totalBytes?: number;
}

export interface PluginGithubRef {
  kind: "branch" | "tag" | "sha";
  value: string;
}

export interface PluginGithubMetadata {
  owner: string;
  repo: string;
  ref: PluginGithubRef;
  installedSha?: string;
}

export interface PluginSummary {
  id: string;
  displayName: string;
  version?: string;
  enabled: boolean;
  state: PluginState;
  skillCount: number;
  mcpServerCount: number;
  enabledMcpServerCount: number;
  hookCount: number;
  commandCount: number;
  hasErrors: boolean;
  source: PluginSource;
  originalSource?: string;
  github?: PluginGithubMetadata;
}

export interface PluginDiagnostic {
  severity: "error" | "warn" | "info";
  message: string;
}

export interface PluginMcpServerInfo {
  name: string;
  runtimeName: string;
  enabled: boolean;
  transport: "stdio" | "http" | "sse";
  command?: string;
  args?: string[];
  cwd?: string;
  url?: string;
  envKeys?: string[];
  headerKeys?: string[];
}

export interface PluginManifest {
  name: string;
  version?: string;
  description?: string;
  keywords?: string[];
  homepage?: string;
  author?: { name?: string; email?: string };
  interface?: {
    displayName?: string;
    shortDescription?: string;
    longDescription?: string;
    developerName?: string;
    websiteURL?: string;
  };
}

export interface PluginInfo extends PluginSummary {
  root: string;
  installedAt: string;
  updatedAt?: string;
  manifestKind?: string;
  manifestPath?: string;
  manifest?: PluginManifest;
  mcpServers: PluginMcpServerInfo[];
  shadowedManifestPath?: string;
  diagnostics: PluginDiagnostic[];
}

export interface PluginUpdateStatus {
  id: string;
  source: PluginSource;
  current?: PluginGithubRef;
  latest: PluginGithubRef;
  displayVersion: string;
  updateAvailable: boolean;
}

export interface PluginMarketplaceEntry {
  id: string;
  displayName: string;
  source: string;
  tier?: PluginMarketplaceTier;
  version?: string;
  description?: string;
  homepage?: string;
  keywords?: string[];
}

export interface PluginMarketplace {
  source: string;
  version?: string;
  plugins: PluginMarketplaceEntry[];
}

export interface ParsedPluginCommand {
  pluginId: string;
  commandName: string;
  args?: string;
}

export interface SlashMenuItem {
  id: string;
  kind: "builtin" | "plugin";
  label: string;
  description: string;
  disabled?: boolean;
  builtin?: "compact" | "fork" | "btw";
  plugin?: PluginCommandDef;
}

interface Semver {
  major: number;
  minor: number;
  patch: number;
  prerelease: string[];
}

function parseSemver(value: string | undefined): Semver | undefined {
  if (!value) return undefined;
  const match = /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(
    value.trim(),
  );
  if (!match) return undefined;
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    prerelease: match[4]?.split(".") ?? [],
  };
}

function comparePrerelease(left: string[], right: string[]): number {
  if (left.length === 0 || right.length === 0) {
    return left.length === right.length ? 0 : left.length === 0 ? 1 : -1;
  }
  for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
    const a = left[index];
    const b = right[index];
    if (a === undefined || b === undefined) return a === b ? 0 : a === undefined ? -1 : 1;
    if (a === b) continue;
    const aNumber = /^\d+$/.test(a) ? Number(a) : undefined;
    const bNumber = /^\d+$/.test(b) ? Number(b) : undefined;
    if (aNumber !== undefined && bNumber !== undefined) return Math.sign(aNumber - bNumber);
    if (aNumber !== undefined) return -1;
    if (bNumber !== undefined) return 1;
    return a.localeCompare(b);
  }
  return 0;
}

export function compareSemver(left: string, right: string): number | undefined {
  const a = parseSemver(left);
  const b = parseSemver(right);
  if (!a || !b) return undefined;
  for (const field of ["major", "minor", "patch"] as const) {
    if (a[field] !== b[field]) return Math.sign(a[field] - b[field]);
  }
  return comparePrerelease(a.prerelease, b.prerelease);
}

export function marketplaceUpdateAvailable(
  installed: PluginSummary | undefined,
  entry: PluginMarketplaceEntry,
): boolean {
  if (!installed?.version || !entry.version) return false;
  return compareSemver(entry.version, installed.version) === 1;
}

export function isThirdPartyEntry(entry: PluginMarketplaceEntry): boolean {
  return entry.tier !== "official";
}

export function pluginTabNeedsNetwork(tab: PluginTab): boolean {
  return tab === "official" || tab === "third-party";
}

export function pluginInstallPercent(
  progress: PluginInstallProgressEvent,
): number | undefined {
  switch (progress.phase) {
    case "resolving":
      return 6;
    case "downloading": {
      if (!progress.totalBytes || progress.totalBytes <= 0) return undefined;
      const ratio = Math.min(1, Math.max(0, progress.downloadedBytes / progress.totalBytes));
      return Math.round(8 + ratio * 64);
    }
    case "extracting":
      return 76;
    case "installing":
      return 90;
    case "complete":
      return 100;
  }
}

export function pluginCommandLabel(command: PluginCommandDef): string {
  return `${command.pluginId}:${command.name}`;
}

export function filterPluginCommands(
  commands: readonly PluginCommandDef[],
  query: string,
): PluginCommandDef[] {
  const normalized = query.trim().replace(/^\//, "").toLowerCase();
  return commands.filter((command) =>
    pluginCommandLabel(command).toLowerCase().includes(normalized),
  );
}

export function parseKnownPluginCommand(
  input: string,
  commands: readonly PluginCommandDef[],
): ParsedPluginCommand | undefined {
  const match = /^\/([^:\s]+):([^\s]+)(?:\s+(.*))?$/.exec(input.trim());
  if (!match) return undefined;
  const command = commands.find(
    (candidate) => candidate.pluginId === match[1] && candidate.name === match[2],
  );
  if (!command) return undefined;
  const args = match[3]?.trim();
  return {
    pluginId: command.pluginId,
    commandName: command.name,
    ...(args ? { args } : {}),
  };
}
