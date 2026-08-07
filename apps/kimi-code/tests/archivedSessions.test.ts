import assert from "node:assert/strict";
import test from "node:test";

import {
  filterArchivedSessions,
  formatArchivedTime,
  groupArchivedSessions,
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

test("archived sessions are grouped by workspace path with a safe fallback", () => {
  const groups = groupArchivedSessions(
    [
      session("one"),
      session("two"),
      session("unknown", { cwd: undefined }),
    ],
    "Unknown workspace",
  );

  assert.deepEqual(
    groups.map((group) => [
      group.path,
      group.sessions.map((item) => item.id),
    ]),
    [
      ["/workspace/one", ["one", "two"]],
      ["Unknown workspace", ["unknown"]],
    ],
  );
  assert.equal(formatArchivedTime(Number.NaN), "—");
});
