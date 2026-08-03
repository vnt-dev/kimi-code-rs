import type { RefObject } from "react";
import { Folder, FolderGit2, Menu } from "lucide-react";

import { t } from "../i18n";

export function ProjectLanding({
  collapsed,
  menuButtonRef,
  onExpand,
  onAddProject,
}: {
  collapsed: boolean;
  menuButtonRef?: RefObject<HTMLButtonElement | null>;
  onExpand: () => void;
  onAddProject: () => void;
}) {
  return (
    <div className="project-landing">
      {collapsed && (
        <button
          className="landing-menu icon-button"
          ref={menuButtonRef}
          type="button"
          aria-label={t("sidebar.expand")}
          onClick={onExpand}
        >
          <Menu size={18} />
        </button>
      )}
      <div className="landing-visual">
        <span className="landing-grid" />
        <div className="landing-folder">
          <FolderGit2 size={42} />
        </div>
        <i className="landing-dot dot-one" />
        <i className="landing-dot dot-two" />
        <i className="landing-dot dot-three" />
      </div>
      <p className="eyebrow">YOUR AI CODING PARTNER</p>
      <h1>{t("landing.title")}</h1>
      <p>
        {t("landing.copy1")}
        <br />
        {t("landing.copy2")}
      </p>
      <button className="landing-primary" onClick={onAddProject}>
        <Folder size={17} />
        {t("landing.openProject")}
      </button>
      <div className="landing-shortcut">
        <span>{t("landing.tip")}</span>
        {t("landing.dragHint")}
      </div>
    </div>
  );
}
