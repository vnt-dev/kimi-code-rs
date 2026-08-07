import assert from "node:assert/strict";
import test from "node:test";

import {
  archivedSessionIdsForWorkspace,
  filterArchivedSessions,
  formatArchivedTime,
  groupArchivedSessions,
  removeArchivedSessions,
} from "../src/archivedSessions.ts";
import type { SessionSummary } from "../src/types.ts";

function session(
  id: string,
  overrides: Partial<SessionSummary> = {},
): SessionSummary {
  return {
    id,
    workspaceId: "workspace-1",
    cwd: "/workspace/one",
    title: id,
    createdAt: 1,
    updatedAt: 2,
    archived: true,
    ...overrides,
  };
}

const baseOptions = {
  query: "",
  workspacePath: "all",
  sort: "archived-desc" as const,
  untitledLabel: "Untitled",
  unknownWorkspaceLabel: "Unknown workspace",
  locale: "en",
};

test("archived settings defensively exclude active sessions and filter globally", () => {
  const sessions = [
    session("alpha", { title: "Alpha", updatedAt: 2 }),
    session("beta", {
      cwd: "/workspace/two",
      title: "Beta result",
      updatedAt: 4,
    }),
    session("active", { archived: false, title: "Beta active", updatedAt: 5 }),
  ];

  assert.deepEqual(
    filterArchivedSessions(sessions, {
      ...baseOptions,
      query: "beta",
      workspacePath: "/workspace/two",
    }).map((item) => item.id),
    ["beta"],
  );
});

test("archived settings support archive time, creation time, and name sorting", () => {
  const sessions = [
    session("bravo", { title: "Bravo", createdAt: 5, updatedAt: 2 }),
    session("alpha", { title: "Alpha", createdAt: 1, updatedAt: 8 }),
  ];

  assert.deepEqual(
    filterArchivedSessions(sessions, baseOptions).map((item) => item.id),
    ["alpha", "bravo"],
  );
  assert.deepEqual(
    filterArchivedSessions(sessions, {
      ...baseOptions,
      sort: "created-desc",
    }).map((item) => item.id),
    ["bravo", "alpha"],
  );
  assert.deepEqual(
    filterArchivedSessions(sessions, {
      ...baseOptions,
      sort: "name-asc",
    }).map((item) => item.id),
    ["alpha", "bravo"],
  );
});

test("archived sessions are grouped by workspace id with a safe path fallback", () => {
  const groups = groupArchivedSessions(
    [
      session("one"),
      session("two"),
      session("unknown", { workspaceId: "workspace-2", cwd: undefined }),
    ],
    "Unknown workspace",
  );

  assert.deepEqual(
    groups.map((group) => [
      group.workspaceId,
      group.path,
      group.sessions.map((item) => item.id),
    ]),
    [
      ["workspace-1", "/workspace/one", ["one", "two"]],
      ["workspace-2", "Unknown workspace", ["unknown"]],
    ],
  );
  assert.equal(formatArchivedTime(Number.NaN), "—");
});

test("workspace deletion targets all archived sessions regardless of search results", () => {
  const sessions = [
    session("visible", { title: "Match" }),
    session("hidden", { title: "Different" }),
    session("other", { workspaceId: "workspace-2" }),
    session("active", { archived: false }),
  ];
  const filtered = filterArchivedSessions(sessions, {
    ...baseOptions,
    query: "match",
  });

  assert.deepEqual(filtered.map((item) => item.id), ["visible"]);
  assert.deepEqual(
    archivedSessionIdsForWorkspace(sessions, "workspace-1"),
    ["visible", "hidden"],
  );
});

test("deleted ids are removed without affecting other archived sessions", () => {
  const sessions = [session("one"), session("two"), session("three")];
  const remaining = removeArchivedSessions(sessions, ["one", "two", "missing"]);

  assert.deepEqual(remaining.map((item) => item.id), ["three"]);
  assert.deepEqual(removeArchivedSessions(sessions, []), sessions);
});
