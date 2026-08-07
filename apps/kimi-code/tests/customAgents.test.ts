import assert from "node:assert/strict";
import test from "node:test";

import {
  customAgentKey,
  newCustomAgentTemplate,
} from "../src/customAgents.ts";
import { setLanguage } from "../src/i18n.ts";

test("new custom agents start from a complete editable template", () => {
  setLanguage("zh");
  const template = newCustomAgentTemplate();

  for (const field of [
    "name:",
    "description:",
    "whenToUse:",
    "override:",
    "model:",
    "tools:",
    "disallowedTools:",
    "subagents:",
  ]) {
    assert.match(template, new RegExp(`^${field}`, "m"));
  }
  assert.match(template, /^\$\{base_prompt\}$/m);
  assert.match(template, /工作要求/);

  setLanguage("en");
  assert.match(newCustomAgentTemplate(), /Requirements:/);
  setLanguage("zh");
});

test("custom agent keys remain unique across application and project scopes", () => {
  assert.equal(
    customAgentKey({ scope: "app", relativePath: "reviewer.md" }),
    "app:reviewer.md",
  );
  assert.equal(
    customAgentKey({ scope: "project", relativePath: "reviewer.md" }),
    "project:reviewer.md",
  );
});
