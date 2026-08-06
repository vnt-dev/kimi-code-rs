import {
  Archive,
  ChevronRight,
  CircleUserRound,
  Folder,
  FolderGit2,
  FolderMinus,
  LogIn,
  MessageSquareText,
  MoreHorizontal,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  Sparkles,
} from "lucide-react";
import {
  type CSSProperties,
  type MouseEvent,
  type RefObject,
} from "react";
import { isTurnRunning, type InFlightTurn } from "../chat/liveTurns";
import { conversationStatus } from "../conversationStatus";
import { t } from "../i18n";
import type {
  AccountUsage,
  AgentInteraction,
  AuthStatus,
  Conversation,
  DesktopState,
  Project,
} from "../types";
import { AccountUsagePopover } from "./AccountUsagePopover";
import type { RemovalTarget } from "./AppDialogs";

interface AppSidebarProps {
  desktop: DesktopState;
  activeProject?: Project;
  activeConversation?: Conversation;
  inFlightTurns: Record<string, InFlightTurn>;
  interactions: Record<string, AgentInteraction[]>;
  unreadCompletedConversations: Record<string, true>;
  auth: AuthStatus;
  appVersion?: string;
  accountUsage?: AccountUsage;
  accountUsageBusy: boolean;
  accountUsageError?: string;
  profileOpen: boolean;
  sidebarCollapsed: boolean;
  mobileLayout: boolean;
  mobileSidebarOpen: boolean;
  profileRef: RefObject<HTMLDivElement | null>;
  onToggleSidebar: () => void;
  onAddProject: () => void;
  onOpenSidebar: () => void;
  onToggleProject: (projectId: string) => void;
  onCreateConversation: (
    project: Project,
    event?: MouseEvent<HTMLButtonElement>,
  ) => void;
  onSelectConversation: (projectId: string, conversationId: string) => void;
  onSetRemovalTarget: (target: RemovalTarget) => void;
  onToggleProfile: () => void;
  onRefreshAccountUsage: () => void;
  onLogin: () => void;
  onOpenSettings: () => void;
  onSignOut: () => void;
  onCloseMobileNavigation: () => void;
}

export function AppSidebar({
  desktop,
  activeProject,
  activeConversation,
  inFlightTurns,
  interactions,
  unreadCompletedConversations,
  auth,
  appVersion,
  accountUsage,
  accountUsageBusy,
  accountUsageError,
  profileOpen,
  sidebarCollapsed,
  mobileLayout,
  mobileSidebarOpen,
  profileRef,
  onToggleSidebar,
  onAddProject,
  onOpenSidebar,
  onToggleProject,
  onCreateConversation,
  onSelectConversation,
  onSetRemovalTarget,
  onToggleProfile,
  onRefreshAccountUsage,
  onLogin,
  onOpenSettings,
  onSignOut,
  onCloseMobileNavigation,
}: AppSidebarProps) {
  return (
    <>
        <aside
          className={sidebarCollapsed ? "sidebar collapsed" : "sidebar"}
          aria-hidden={mobileLayout && !mobileSidebarOpen}
          inert={mobileLayout && !mobileSidebarOpen}
        >
        <div className="brand-row">
          <div className="sidebar-heading-copy" aria-hidden={sidebarCollapsed}>
            <strong>{t("sidebar.workspace")}</strong>
          </div>
          <button
            className="icon-button quiet"
            type="button"
            aria-label={
              sidebarCollapsed ? t("sidebar.expand") : t("sidebar.collapse")
            }
            aria-expanded={!sidebarCollapsed}
            onClick={onToggleSidebar}
            title={sidebarCollapsed ? t("sidebar.expand") : t("sidebar.collapse")}
          >
            {sidebarCollapsed ? (
              <PanelLeftOpen size={17} />
            ) : (
              <PanelLeftClose size={17} />
            )}
          </button>
        </div>

        <div className="sidebar-primary">
          <button className="new-project-button" onClick={() => onAddProject()}>
            <Plus size={17} />
            <span className="sidebar-control-label" aria-hidden={sidebarCollapsed}>
              {t("sidebar.openProject")}
            </span>
          </button>

          <div className="sidebar-section-heading" aria-hidden={sidebarCollapsed}>
            <span>{t("sidebar.projects")}</span>
          </div>

          <nav className="project-list" aria-label={t("sidebar.projectsAndConversations")}>
            {desktop.projects.map((project) => {
              const isProjectActive = project.id === activeProject?.id;
              return (
                <div
                  className={`project-group ${isProjectActive ? "active" : ""}`}
                  key={project.id}
                >
                  <div
                    className="project-row"
                    onClick={() =>
                      sidebarCollapsed
                        ? onOpenSidebar()
                        : onToggleProject(project.id)
                    }
                    title={project.path}
                  >
                    <span
                      className="project-glyph"
                      style={{ "--project-accent": project.accent } as CSSProperties}
                    >
                      <FolderGit2 size={16} />
                    </span>
                    <span className="project-name" aria-hidden={sidebarCollapsed}>
                      {project.name}
                    </span>
                    <span className="project-actions" aria-hidden={sidebarCollapsed}>
                      <button
                        className="icon-button tiny"
                        type="button"
                        tabIndex={sidebarCollapsed ? -1 : 0}
                        onClick={(event) =>
                          onCreateConversation(project, event)
                        }
                        title={t("conversation.create")}
                        aria-label={t("conversation.newIn", { name: project.name })}
                      >
                        <Plus size={14} />
                      </button>
                      <button
                        className="icon-button tiny project-remove-button"
                        type="button"
                        tabIndex={sidebarCollapsed ? -1 : 0}
                        onClick={(event) => {
                          event.stopPropagation();
                          onSetRemovalTarget({
                            kind: "project",
                            projectId: project.id,
                            name: project.name,
                            path: project.path,
                            conversationIds: project.conversations.map(
                              (conversation) => conversation.id,
                            ),
                          });
                        }}
                        title={t("sidebar.removeProject")}
                        aria-label={t("sidebar.removeProjectNamed", { name: project.name })}
                      >
                        <FolderMinus size={13} />
                      </button>
                      <ChevronRight
                        className={`project-chevron ${
                          project.expanded ? "expanded" : ""
                        }`}
                        size={14}
                      />
                    </span>
                  </div>
                  <div
                    className={`conversation-list-collapse ${
                      !sidebarCollapsed && project.expanded ? "expanded" : ""
                    }`}
                    aria-hidden={sidebarCollapsed || !project.expanded}
                    inert={sidebarCollapsed || !project.expanded}
                  >
                    <div className="conversation-list-clip">
                      <div className="conversation-list">
                        {project.conversations.map((conversation) => {
                          const status = conversationStatus({
                            interactions: interactions[conversation.id],
                            running: isTurnRunning(
                              inFlightTurns[conversation.id],
                            ),
                            completedUnread:
                              unreadCompletedConversations[conversation.id] ===
                              true,
                          });
                          const statusLabel =
                            status === "attention"
                              ? t("conversation.needsAttention")
                              : status === "running"
                                ? t("conversation.running")
                                : t("conversation.completedUnread");
                          return (
                            <div
                              className={`conversation-row ${
                                conversation.id === activeConversation?.id
                                  ? "selected"
                                  : ""
                              }`}
                              key={conversation.id}
                            >
                              <button
                                className="conversation-select"
                                type="button"
                                onClick={() =>
                                  onSelectConversation(
                                    project.id,
                                    conversation.id,
                                  )
                                }
                                title={conversation.title}
                              >
                                <MessageSquareText size={14} />
                                <span className="conversation-title">
                                  {conversation.title}
                                </span>
                              </button>
                              <span className="conversation-status-slot">
                                {status && (
                                  <span
                                    className={`conversation-status-indicator ${status}`}
                                    role="status"
                                    aria-label={statusLabel}
                                    title={statusLabel}
                                  />
                                )}
                                <button
                                  className="conversation-archive-button"
                                  type="button"
                                  onClick={(event) => {
                                    event.stopPropagation();
                                    onSetRemovalTarget({
                                      kind: "conversation",
                                      projectId: project.id,
                                      conversationId: conversation.id,
                                      title: conversation.title,
                                    });
                                  }}
                                  title={t("conversation.archive")}
                                  aria-label={t("conversation.archiveNamed", {
                                    title: conversation.title,
                                  })}
                                >
                                  <Archive size={12} />
                                </button>
                              </span>
                            </div>
                          );
                        })}
                      </div>
                    </div>
                  </div>
                </div>
              );
            })}
          </nav>

          {desktop.projects.length === 0 && (
            <div className="sidebar-empty" aria-hidden={sidebarCollapsed}>
              <Folder size={22} />
              <p>{t("sidebar.empty")}</p>
            </div>
          )}
        </div>

        <div className="account-area">
          <div className="profile-wrap" ref={profileRef}>
            <button
              className={auth.loggedIn ? "account-button" : "account-button login"}
              tabIndex={sidebarCollapsed ? -1 : 0}
              aria-label={t("account.openMenu")}
              aria-expanded={profileOpen}
              aria-controls="account-popover"
              onClick={onToggleProfile}
            >
              <span className="account-copy" aria-hidden={sidebarCollapsed}>
                <strong>
                  {auth.loggedIn ? "Kimi Code" : t("account.login")}
                </strong>
                <small>
                  {auth.loggedIn
                    ? t("account.connected")
                    : t("account.loginHint")}
                </small>
              </span>
              {auth.loggedIn ? (
                <MoreHorizontal
                  className="account-trailing-icon"
                  size={16}
                  aria-hidden={sidebarCollapsed}
                />
              ) : (
                <LogIn
                  className="account-trailing-icon"
                  size={16}
                  aria-hidden={sidebarCollapsed}
                />
              )}
            </button>
            <div
              className="account-compact-actions"
              aria-hidden={!sidebarCollapsed}
              inert={!sidebarCollapsed}
            >
              <button
                className="account-compact-kimi"
                type="button"
                title={t("account.kimiAccount")}
                aria-label={t("account.openMenu")}
                aria-expanded={profileOpen}
                aria-controls="account-popover"
                onClick={onToggleProfile}
              >
                {auth.loggedIn ? (
                  <Sparkles size={14} />
                ) : (
                  <CircleUserRound size={15} />
                )}
              </button>
            </div>
            {profileOpen && (
              <AccountUsagePopover
                appVersion={appVersion}
                loggedIn={auth.loggedIn}
                usage={accountUsage}
                busy={accountUsageBusy}
                error={accountUsageError}
                onRefresh={() => onRefreshAccountUsage()}
                onLogin={() => onLogin()}
                onOpenSettings={onOpenSettings}
                onSignOut={() => onSignOut()}
              />
            )}
          </div>
        </div>
        </aside>

        {mobileLayout && mobileSidebarOpen && (
          <button
            className="mobile-sidebar-backdrop"
            type="button"
            aria-label={t("sidebar.collapse")}
            onClick={onCloseMobileNavigation}
          />
        )}

    </>
  );
}
