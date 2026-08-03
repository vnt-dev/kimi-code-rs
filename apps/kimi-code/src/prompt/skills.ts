import type { SkillDescriptor } from "../types";

export interface SkillPromptDisplay {
  text: string;
  skills: string[];
}

export function buildSkillPromptText(
  text: string,
  skills: readonly SkillDescriptor[],
): string {
  const mentions = skills.map((skill) => `$${skill.name}`).join(" ");
  return [mentions, text].filter(Boolean).join(" ");
}

function decodeSkillAttribute(value: string): string {
  return value
    .replaceAll("&quot;", '"')
    .replaceAll("&apos;", "'")
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&amp;", "&");
}

export function parseSkillPromptDisplay(value: string): SkillPromptDisplay {
  const skills: string[] = [];
  const collectSkills = (content: string): void => {
    const pattern =
      /<kimi-skill-loaded\b[^>]*\bname=(["'])(.*?)\1[^>]*>/gi;
    for (const match of content.matchAll(pattern)) {
      const name = decodeSkillAttribute(match[2]).trim();
      if (name && !skills.includes(name)) skills.push(name);
    }
  };

  let text = value.replace(
    /<kimi-selected-skills\b[^>]*>[\s\S]*?<\/kimi-selected-skills>\s*/gi,
    (block) => {
      collectSkills(block);
      return "";
    },
  );
  text = text.replace(
    /<kimi-skill-loaded\b[^>]*>[\s\S]*?<\/kimi-skill-loaded>\s*/gi,
    (block) => {
      collectSkills(block);
      return "";
    },
  );
  text = text.replace(
    /User activated the skill "[^"]+"\.\s*Follow the loaded skill instructions\.\s*/gi,
    "",
  );
  return {
    text: text.trim().replace(/\n{3,}/g, "\n\n"),
    skills,
  };
}
