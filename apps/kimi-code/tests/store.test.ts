import assert from "node:assert/strict";
import test from "node:test";

import {
  initialDesktopState,
  mergeDesktopInventory,
} from "../src/desktopInventory.ts";
import type { DesktopState, Project, Workspace } from "../src/types.ts";

test("initial desktop state opens and expands only the latest workspace", () => {
  const projects: Project[] = [
    {
      id: "workspace-1",
      name: "one",
      path: "/one",
      accent: "one",
      expanded: true,
      conversations: [
        { id: "session-1", title: "one", createdAt: 1, updatedAt: 100 },
      ],
    },
    {
      id: "workspace-2",
      name: "two",
      path: "/two",
      accent: "two",
      expanded: true,
      conversations: [
        { id: "session-2", title: "latest", createdAt: 2, updatedAt: 20 },
        { id: "session-3", title: "older", createdAt: 3, updatedAt: 10 },
      ],
    },
  ];
  const workspaces: Workspace[] = [
    {
      id: "workspace-1",
      root: "/one",
      name: "one",
      createdAt: 1,
      lastOpenedAt: 10,
    },
    {
      id: "workspace-2",
      root: "/two",
      name: "two",
      createdAt: 2,
      lastOpenedAt: 20,
    },
  ];

  const state = initialDesktopState(projects, workspaces);
  assert.equal(state.activeProjectId, "workspace-2");
  assert.equal(state.activeConversationId, "session-2");
  assert.deepEqual(
    state.projects.map((project) => project.expanded),
    [false, true],
  );
});

test("initial desktop state handles an empty inventory", () => {
  assert.deepEqual(initialDesktopState([], []), {
    projects: [],
    activeProjectId: undefined,
    activeConversationId: undefined,
  });
});

test("desktop inventory refresh adds remote workspaces and sessions without changing selection", () => {
  const current: DesktopState = {
    projects: [
      {
        id: "workspace-1",
        name: "one",
        path: "/one",
        accent: "custom",
        expanded: false,
        conversations: [
          {
            id: "session-1",
            title: "active",
            createdAt: 1,
            updatedAt: 1,
            modelId: "model-1",
            thinkingLevel: "high",
            permissionMode: "auto",
          },
        ],
      },
    ],
    activeProjectId: "workspace-1",
    activeConversationId: "session-1",
  };
  const incoming: DesktopState = {
    projects: [
      {
        id: "workspace-1",
        name: "one",
        path: "/one",
        accent: "generated",
        expanded: true,
        conversations: [
          { id: "session-2", title: "remote", createdAt: 2, updatedAt: 2 },
          { id: "session-1", title: "active", createdAt: 1, updatedAt: 3 },
        ],
      },
      {
        id: "workspace-2",
        name: "two",
        path: "/two",
        accent: "second",
        expanded: true,
        conversations: [],
      },
    ],
    activeProjectId: "workspace-1",
    activeConversationId: "session-2",
  };

  const merged = mergeDesktopInventory(current, incoming);

  assert.equal(merged.projects.length, 2);
  assert.equal(merged.projects[0].expanded, false);
  assert.equal(merged.projects[0].accent, "custom");
  assert.equal(merged.projects[0].conversations[1].modelId, "model-1");
  assert.equal(merged.activeProjectId, "workspace-1");
  assert.equal(merged.activeConversationId, "session-1");
});

test("desktop inventory refresh selects a valid fallback after removal", () => {
  const current: DesktopState = {
    projects: [
      {
        id: "removed",
        name: "removed",
        path: "/removed",
        accent: "old",
        expanded: true,
        conversations: [
          { id: "old", title: "old", createdAt: 1, updatedAt: 1 },
        ],
      },
    ],
    activeProjectId: "removed",
    activeConversationId: "old",
  };
  const incoming: DesktopState = {
    projects: [
      {
        id: "remaining",
        name: "remaining",
        path: "/remaining",
        accent: "new",
        expanded: true,
        conversations: [
          { id: "next", title: "next", createdAt: 2, updatedAt: 2 },
        ],
      },
    ],
    activeProjectId: "remaining",
    activeConversationId: "next",
  };

  const merged = mergeDesktopInventory(current, incoming);
  assert.equal(merged.activeProjectId, "remaining");
  assert.equal(merged.activeConversationId, "next");
});
