import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("keeps updater state mounted while the settings dialog is closed", async () => {
  const overlaysSource = await readFile(
    new URL("../src/components/AppOverlays.tsx", import.meta.url),
    "utf8",
  );
  const dialogSource = await readFile(
    new URL("../src/SettingsDialog.tsx", import.meta.url),
    "utf8",
  );

  assert.match(overlaysSource, /<SettingsDialog\s+open=\{settingsOpen\}/);
  assert.doesNotMatch(
    overlaysSource,
    /\{settingsOpen\s*&&\s*\(\s*<SettingsDialog/,
  );
  assert.match(dialogSource, /if \(!open\) return null;/);
  assert.match(dialogSource, /if \(!open\) return;[\s\S]*document\.activeElement/);
});
