import { Package } from "lucide-react";

import { t } from "../i18n";
import { parseSkillPromptDisplay } from "./skills";

function SkillNameChips({
  names,
  onSkillOpen,
}: {
  names: readonly string[];
  onSkillOpen?: (name: string) => void;
}) {
  if (names.length === 0) return null;
  return (
    <div className="message-skill-list" aria-label={t("skills.usedInMessage")}>
      {names.map((name) =>
        onSkillOpen ? (
          <button
            className="message-skill-chip"
            type="button"
            title={t("skills.viewDetail")}
            aria-label={t("skills.viewSkill", { name })}
            key={name}
            onClick={() => onSkillOpen(name)}
          >
            <Package size={13} />
            {name}
          </button>
        ) : (
          <span className="message-skill-chip" key={name}>
            <Package size={13} />
            {name}
          </span>
        ),
      )}
    </div>
  );
}

export function SkillPromptDisplayContent({
  text,
  skills = [],
  onSkillOpen,
}: {
  text: string;
  skills?: readonly string[];
  onSkillOpen?: (name: string) => void;
}) {
  const parsed = parseSkillPromptDisplay(text);
  const names = [...skills];
  for (const name of parsed.skills) {
    if (!names.includes(name)) names.push(name);
  }
  return (
    <>
      <SkillNameChips names={names} onSkillOpen={onSkillOpen} />
      {parsed.text}
    </>
  );
}
