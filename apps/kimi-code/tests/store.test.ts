import assert from "node:assert/strict";
import test from "node:test";

import { mergeDesktopInventory } from "../src/desktopInventory.ts";
import type { DesktopState } from "../src/types.ts";

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
