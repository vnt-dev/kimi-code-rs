import { getLanguage, type Language } from "./i18n.ts";
import type { SkillDescriptor } from "./types.ts";

const FEATURED_EXTRA_SKILLS = [
  "kimi-cu",
  "kimi-webbridge",
  "kimi-datasource",
] as const;

const ZH_SKILL_DISPLAY_NAMES: Readonly<Record<string, string>> = {
  "kimi-cu": "电脑控制(kimi-cu)",
  "kimi-webbridge": "浏览器控制(kimi-webbridge)",
  "kimi-datasource": "通用数据源(kimi-datasource)",
};

export function skillDisplayName(
  name: string,
  language: Language = getLanguage(),
): string {
  return language === "zh" ? (ZH_SKILL_DISPLAY_NAMES[name] ?? name) : name;
}

export function sortSkillsForAddMenu(
  skills: readonly SkillDescriptor[],
): SkillDescriptor[] {
  return skills
    .map((skill, index) => ({ skill, index }))
    .sort((left, right) => {
      const rank = skillMenuRank(left.skill) - skillMenuRank(right.skill);
      return rank || left.index - right.index;
    })
    .map(({ skill }) => skill);
}

function skillMenuRank(skill: SkillDescriptor): number {
  if (skill.source === "extra") {
    const featuredIndex = FEATURED_EXTRA_SKILLS.indexOf(
      skill.name as (typeof FEATURED_EXTRA_SKILLS)[number],
    );
    if (featuredIndex >= 0) return featuredIndex;
  }
  switch (skill.source) {
    case "user":
      return 100;
    case "project":
      return 200;
    case "extra":
      return 300;
    case "builtin":
      return 400;
  }
}
