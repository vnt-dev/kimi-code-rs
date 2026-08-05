import assert from "node:assert/strict";
import test from "node:test";

import type { PluginCommandDef } from "../src/agentRpc.ts";
import {
  compareSemver,
  filterPluginCommands,
  isThirdPartyEntry,
  marketplaceUpdateAvailable,
  parseKnownPluginCommand,
  pluginInstallPercent,
  type PluginMarketplaceEntry,
  type PluginSummary,
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

test("plugin install progress uses real download bytes when total size is known", () => {
  assert.equal(
    pluginInstallPercent({
      operationId: "install-1",
      phase: "downloading",
      downloadedBytes: 50,
      totalBytes: 100,
    }),
    40,
  );
  assert.equal(
    pluginInstallPercent({
      operationId: "install-1",
      phase: "downloading",
      downloadedBytes: 50,
    }),
    undefined,
  );
  assert.equal(
    pluginInstallPercent({
      operationId: "install-1",
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
