import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const START_MARKER = "<!-- kimi-code-downloads:start -->";
const END_MARKER = "<!-- kimi-code-downloads:end -->";

const DOWNLOADS = [
  {
    platform: "Linux",
    architecture: "x86_64 / AMD64",
    format: "AppImage",
    label: "Download AppImage",
    filename: (version) => `Kimi.Code_${version}_linux_amd64.AppImage`,
  },
  {
    platform: "Linux",
    architecture: "x86_64 / AMD64",
    format: "DEB",
    label: "Download DEB",
    filename: (version) => `Kimi.Code_${version}_linux_amd64.deb`,
  },
  {
    platform: "macOS",
    architecture: "Apple Silicon / ARM64",
    format: "DMG",
    label: "Download macOS ARM64",
    filename: (version) => `Kimi.Code_${version}_macos_aarch64.dmg`,
  },
  {
    platform: "macOS",
    architecture: "Intel / x86_64",
    format: "DMG",
    label: "Download macOS Intel",
    filename: (version) => `Kimi.Code_${version}_macos_x64.dmg`,
  },
  {
    platform: "Windows",
    architecture: "x86_64 / AMD64",
    format: "EXE installer",
    label: "Download EXE",
    filename: (version) => `Kimi.Code_${version}_windows_x64-setup.exe`,
  },
  {
    platform: "Windows",
    architecture: "x86_64 / AMD64",
    format: "MSI installer",
    label: "Download MSI",
    filename: (version) => `Kimi.Code_${version}_windows_x64.msi`,
  },
];

export function buildDownloadSection({ version, releaseUrl, assetNames }) {
  const rows = DOWNLOADS.flatMap((download) => {
    const filename = download.filename(version);
    if (!assetNames.has(filename)) return [];
    const url = `${releaseUrl}/${encodeURIComponent(filename)}`;
    return [
      `| ${download.platform} | ${download.architecture} | ${download.format} | [${download.label}](${url}) |`,
    ];
  });

  if (rows.length === 0) return undefined;

  return [
    START_MARKER,
    "## Downloads",
    "",
    "| Platform | Architecture | Package format | Download |",
    "|---|---|---|---|",
    ...rows,
    END_MARKER,
  ].join("\n");
}

export function mergeDownloadSection(notes, section) {
  const start = notes.indexOf(START_MARKER);
  if (start === -1) {
    const existing = notes.trimEnd();
    return existing.length === 0 ? `${section}\n` : `${existing}\n\n${section}\n`;
  }

  const end = notes.indexOf(END_MARKER, start);
  if (end === -1) {
    throw new Error("release notes contain an incomplete download section");
  }

  return `${notes.slice(0, start)}${section}${notes.slice(end + END_MARKER.length)}`;
}

function releaseAssetId(url) {
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return undefined;
  }
  if (parsed.hostname !== "api.github.com") return undefined;
  return parsed.pathname.match(/\/releases\/assets\/(\d+)$/)?.[1];
}

export function normalizeUpdaterManifest(manifest, assets) {
  const downloadUrls = new Map(
    assets.map((asset) => [String(asset.id), asset.browser_download_url]),
  );
  let changed = false;
  const platforms = Object.fromEntries(
    Object.entries(manifest.platforms || {}).map(([target, platform]) => {
      const assetId = releaseAssetId(platform.url);
      if (!assetId) return [target, platform];

      const downloadUrl = downloadUrls.get(assetId);
      if (!downloadUrl) {
        throw new Error(
          `updater platform ${target} references unknown release asset ${assetId}`,
        );
      }
      changed = true;
      return [target, { ...platform, url: downloadUrl }];
    }),
  );

  return {
    changed,
    manifest: changed ? { ...manifest, platforms } : manifest,
  };
}

function normalizeReleaseUpdaterManifest({ repository, tag, release }) {
  const latestAsset = release.assets.find((asset) => asset.name === "latest.json");
  if (!latestAsset) {
    throw new Error(`release ${tag} does not contain latest.json`);
  }

  const manifest = JSON.parse(
    execFileSync(
      "gh",
      [
        "api",
        "-H",
        "Accept: application/octet-stream",
        `repos/${repository}/releases/assets/${latestAsset.id}`,
      ],
      { encoding: "utf8", env: process.env, windowsHide: true },
    ),
  );
  const normalized = normalizeUpdaterManifest(manifest, release.assets);
  if (!normalized.changed) {
    console.log(`Release ${tag} updater links are already normalized.`);
    return;
  }

  const manifestDir = mkdtempSync(join(tmpdir(), "kimi-updater-manifest-"));
  const manifestPath = join(manifestDir, "latest.json");
  try {
    writeFileSync(manifestPath, `${JSON.stringify(normalized.manifest, null, 2)}\n`);
    execFileSync(
      "gh",
      ["release", "upload", tag, manifestPath, "--repo", repository, "--clobber"],
      { stdio: "inherit", env: process.env, windowsHide: true },
    );
  } finally {
    rmSync(manifestDir, { recursive: true, force: true });
  }

  console.log(`Normalized updater download links for release ${tag}.`);
}

function requiredEnv(name) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function main() {
  const repository = requiredEnv("GITHUB_REPOSITORY");
  const tag = requiredEnv("GITHUB_REF_NAME");
  const serverUrl = requiredEnv("GITHUB_SERVER_URL").replace(/\/$/, "");
  const workspace = process.env.GITHUB_WORKSPACE || process.cwd();
  const configPath = resolve(workspace, "apps/kimi-code/src-tauri/tauri.conf.json");
  const version = JSON.parse(readFileSync(configPath, "utf8")).version;
  const release = JSON.parse(
    execFileSync(
      "gh",
      ["api", `repos/${repository}/releases/tags/${tag}`],
      { encoding: "utf8", env: process.env, windowsHide: true },
    ),
  );
  normalizeReleaseUpdaterManifest({ repository, tag, release });
  const assetNames = new Set(release.assets.map((asset) => asset.name));
  const releaseUrl = `${serverUrl}/${repository}/releases/download/${tag}`;
  const section = buildDownloadSection({ version, releaseUrl, assetNames });

  if (!section) {
    throw new Error(`release ${tag} does not contain a recognized desktop installer`);
  }

  const notes = release.body || "";
  const updatedNotes = mergeDownloadSection(notes, section);
  if (updatedNotes === notes) {
    console.log(`Release ${tag} download links are already up to date.`);
    return;
  }

  const notesFile = join(
    process.env.RUNNER_TEMP || tmpdir(),
    `kimi-release-notes-${process.pid}.md`,
  );
  try {
    writeFileSync(notesFile, updatedNotes);
    execFileSync(
      "gh",
      ["release", "edit", tag, "--repo", repository, "--notes-file", notesFile],
      { stdio: "inherit", env: process.env, windowsHide: true },
    );
  } finally {
    if (existsSync(notesFile)) unlinkSync(notesFile);
  }

  console.log(`Updated release ${tag} download links.`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main();
}
