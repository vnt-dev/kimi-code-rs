import assert from "node:assert/strict";
import test from "node:test";

import {
  skillDisplayName,
  sortSkillsForAddMenu,
} from "../src/skillDisplayName.ts";
import type { SkillDescriptor } from "../src/types.ts";

test("skill names are localized only for the Chinese add menu", () => {
  assert.equal(skillDisplayName("kimi-cu", "zh"), "电脑控制(kimi-cu)");
  assert.equal(
    skillDisplayName("kimi-webbridge", "zh"),
    "浏览器控制(kimi-webbridge)",
  );
  assert.equal(
    skillDisplayName("kimi-datasource", "zh"),
    "通用数据源(kimi-datasource)",
  );
  assert.equal(skillDisplayName("other-skill", "zh"), "other-skill");
  assert.equal(skillDisplayName("kimi-cu", "en"), "kimi-cu");
});

test("add menu prioritizes featured extras, users, and builtins in that order", () => {
  const skill = (
    name: string,
    source: SkillDescriptor["source"],
  ): SkillDescriptor => ({ name, source, description: name });
  const skills = [
    skill("builtin-a", "builtin"),
    skill("project-a", "project"),
    skill("other-extra", "extra"),
    skill("user-a", "user"),
    skill("kimi-datasource", "extra"),
    skill("kimi-cu", "extra"),
    skill("user-b", "user"),
    skill("kimi-webbridge", "extra"),
  ];

  assert.deepEqual(
    sortSkillsForAddMenu(skills).map(({ name }) => name),
    [
      "kimi-cu",
      "kimi-webbridge",
      "kimi-datasource",
      "user-a",
      "user-b",
      "project-a",
      "other-extra",
      "builtin-a",
    ],
  );
  assert.equal(skills[0].name, "builtin-a");
});
