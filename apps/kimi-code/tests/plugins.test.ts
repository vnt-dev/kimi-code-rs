import assert from "node:assert/strict";
import test from "node:test";

import type { PluginCommandDef } from "../src/agentRpc.ts";
import {
  compareSemver,
  filterPluginCommands,
  isThirdPartyEntry,
  maskCapabilityMarketplaceEntries,
  marketplaceEntryForCapability,
  marketplaceUpdateAvailable,
  parseKnownPluginCommand,
  pluginInstallPercent,
  pluginTabNeedsNetwork,
  type PluginMarketplaceEntry,
  type PluginSummary,
  type CapabilityStatus,
} from "../src/plugins.ts";

const installed: PluginSummary = {
  id: "demo",
  displayName: "Demo",
  version: "1.2.3",
  enabled: true,
  state: "ok",
  skillCount: 1,
  mcpServerCount: 0,
  enabledMcpServerCount: 0,
  hookCount: 0,
  commandCount: 1,
  hasErrors: false,
  source: "zip-url",
};

const entry: PluginMarketplaceEntry = {
  id: "demo",
  displayName: "Demo",
  version: "1.3.0",
  tier: "official",
  source: "https://example.test/demo.zip",
};

const commands: PluginCommandDef[] = [
  {
    pluginId: "demo",
    name: "review",
    description: "Review changes",
    body: "Review $ARGUMENTS",
    path: "/plugins/demo/review.md",
  },
  {
    pluginId: "tools",
    name: "deploy",
    description: "Deploy",
    body: "Deploy",
    path: "/plugins/tools/deploy.md",
  },
];

test("semantic version comparison only offers newer marketplace versions", () => {
  assert.equal(compareSemver("1.3.0", "1.2.3"), 1);
  assert.equal(compareSemver("1.2.3-beta.1", "1.2.3"), -1);
  assert.equal(compareSemver("invalid", "1.2.3"), undefined);
  assert.equal(marketplaceUpdateAvailable(installed, entry), true);
  assert.equal(
    marketplaceUpdateAvailable(installed, { ...entry, version: "1.2.2" }),
    false,
  );
});

test("only official marketplace entries bypass third-party confirmation", () => {
  assert.equal(isThirdPartyEntry(entry), false);
  assert.equal(isThirdPartyEntry({ ...entry, tier: "curated" }), true);
  assert.equal(isThirdPartyEntry({ ...entry, tier: undefined }), true);
});

test("installed and custom plugin tabs do not require network data", () => {
  assert.equal(pluginTabNeedsNetwork("installed"), false);
  assert.equal(pluginTabNeedsNetwork("custom"), false);
  assert.equal(pluginTabNeedsNetwork("official"), true);
  assert.equal(pluginTabNeedsNetwork("third-party"), true);
});

test("built-in capabilities mask same-id marketplace rows", () => {
  const capability: CapabilityStatus = {
    id: "kimi-webbridge",
    pluginId: "kimi-webbridge",
    displayName: "Kimi WebBridge",
    description: "Browser control",
    supported: true,
    state: "not_installed",
    steps: [],
    install: { running: false },
  };
  const entries: PluginMarketplaceEntry[] = [
    {
      id: "kimi-webbridge",
      displayName: "Kimi WebBridge",
      tier: "official",
      version: "1.11.3",
      source: "https://example.test/kimi-webbridge.zip",
    },
    entry,
  ];

  assert.deepEqual(maskCapabilityMarketplaceEntries(entries, [capability]), [entry]);
  assert.equal(marketplaceEntryForCapability(entries, capability)?.version, "1.11.3");
});

test("capability plugin ids also mask their marketplace wiring rows", () => {
  const capability: CapabilityStatus = {
    id: "kimi-cu",
    pluginId: "kimi-cu-win",
    displayName: "Kimi Computer Use",
    description: "Computer control",
    supported: false,
    state: "unsupported",
    steps: [],
    install: { running: false },
  };
  const entries: PluginMarketplaceEntry[] = [
    { ...entry, id: "kimi-cu", displayName: "Kimi CU" },
    { ...entry, id: "kimi-cu-win", displayName: "Kimi CU Windows" },
    entry,
  ];

  assert.deepEqual(maskCapabilityMarketplaceEntries(entries, [capability]), [entry]);
  assert.equal(marketplaceEntryForCapability(entries, capability)?.id, "kimi-cu");
});

test("plugin install progress uses real download bytes when total size is known", () => {
  assert.equal(
    pluginInstallPercent({
      operationId: "install-1",
      source: "demo.zip",
      phase: "downloading",
      downloadedBytes: 50,
      totalBytes: 100,
    }),
    40,
  );
  assert.equal(
    pluginInstallPercent({
      operationId: "install-1",
      source: "demo.zip",
      phase: "downloading",
      downloadedBytes: 50,
    }),
    undefined,
  );
  assert.equal(
    pluginInstallPercent({
      operationId: "install-1",
      source: "demo.zip",
      phase: "complete",
      downloadedBytes: 0,
    }),
    100,
  );
});

test("plugin commands filter by namespaced name and parse arguments", () => {
  assert.deepEqual(filterPluginCommands(commands, "/demo"), [commands[0]]);
  assert.deepEqual(parseKnownPluginCommand("/demo:review src/app.ts", commands), {
    pluginId: "demo",
    commandName: "review",
    args: "src/app.ts",
  });
  assert.equal(parseKnownPluginCommand("/demo:missing", commands), undefined);
  assert.equal(parseKnownPluginCommand("explain /demo:review", commands), undefined);
});
