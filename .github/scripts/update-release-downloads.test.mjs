import assert from "node:assert/strict";
import test from "node:test";

import { normalizeUpdaterManifest } from "./update-release-downloads.mjs";

test("replaces GitHub API updater URLs with public release download URLs", () => {
  const manifest = {
    version: "1.2.3",
    platforms: {
      "windows-x86_64": {
        signature: "windows-signature",
        url: "https://api.github.com/repos/example/app/releases/assets/101",
      },
      "linux-x86_64": {
        signature: "linux-signature",
        url: "https://github.com/example/app/releases/download/v1.2.3/app.AppImage",
      },
    },
  };
  const assets = [
    {
      id: 101,
      name: "app.msi",
      browser_download_url:
        "https://github.com/example/app/releases/download/v1.2.3/app.msi",
    },
  ];

  const normalized = normalizeUpdaterManifest(manifest, assets);

  assert.equal(normalized.changed, true);
  assert.equal(
    normalized.manifest.platforms["windows-x86_64"].url,
    assets[0].browser_download_url,
  );
  assert.equal(
    normalized.manifest.platforms["windows-x86_64"].signature,
    "windows-signature",
  );
  assert.strictEqual(
    normalized.manifest.platforms["linux-x86_64"],
    manifest.platforms["linux-x86_64"],
  );
});

test("rejects updater API URLs that do not belong to a release asset", () => {
  assert.throws(
    () =>
      normalizeUpdaterManifest(
        {
          platforms: {
            "windows-x86_64": {
              signature: "signature",
              url: "https://api.github.com/repos/example/app/releases/assets/999",
            },
          },
        },
        [],
      ),
    /unknown release asset 999/,
  );
});
