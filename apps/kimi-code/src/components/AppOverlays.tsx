import { X } from "lucide-react";
import type { ColorScheme } from "../appearance";
import type { RenderMessage } from "../chat/history";
import type { Language } from "../i18n";
import { t } from "../i18n";
import SettingsDialog from "../SettingsDialog";
import { CustomAgentManagerDialog } from "./CustomAgentManagerDialog";
import { isDesktop } from "../transport";
import type {
  AccountProfile,
  AccountUsage,
  AuthStatus,
  DeviceCode,
  GoalSnapshot,
} from "../types";
import {
  DirectoryPickerDialog,
  GoalEditDialog,
  LoginDialog,
  RemovalDialog,
  UndoMessageDialog,
  WebCredentialDialog,
  type RemovalTarget,
} from "./AppDialogs";

interface AppOverlaysProps {
  loginOpen: boolean;
  loginBusy: boolean;
  deviceCode?: DeviceCode;
  webAuthOpen: boolean;
  removalTarget?: RemovalTarget;
  removalBusy: boolean;
  undoMessageTarget?: RenderMessage;
  undoMessageBusy: boolean;
  goalEditTarget?: GoalSnapshot;
  goalEditBusy: boolean;
  directoryPickerOpen: boolean;
  settingsOpen: boolean;
  agentManagerOpen: boolean;
  agentManagerWorkspace?: { id: string; name: string };
  appVersion?: string;
  auth: AuthStatus;
  accountProfile?: AccountProfile;
  accountUsage?: AccountUsage;
  accountUsageBusy: boolean;
  accountUsageError?: string;
  colorScheme: ColorScheme;
  language: Language;
  notificationsEnabled: boolean;
  notice?: string;
  onCloseLogin: () => void;
  onStartLogin: () => void;
  onRefreshAccountUsage: () => void;
  onSignOut: () => void;
  onSubmitCredential: (credential: string) => void;
  onCloseRemoval: () => void;
  onConfirmRemoval: () => void;
  onCloseUndoMessage: () => void;
  onConfirmUndoMessage: () => void;
  onCloseGoalEdit: () => void;
  onConfirmGoalEdit: (goal: GoalSnapshot, objective: string) => void;
  onCloseDirectoryPicker: () => void;
  onSelectDirectory: (path: string) => void;
  onColorSchemeChange: (colorScheme: ColorScheme) => void;
  onLanguageChange: (language: Language) => void;
  onNotificationsEnabledChange: (enabled: boolean) => Promise<void>;
  onProvidersChanged: () => void;
  onPluginsChanged: () => void;
  onCloseSettings: () => void;
  onCloseAgentManager: () => void;
  onDismissNotice: () => void;
}

export function AppOverlays({
  loginOpen,
  loginBusy,
  deviceCode,
  webAuthOpen,
  removalTarget,
  removalBusy,
  undoMessageTarget,
  undoMessageBusy,
  goalEditTarget,
  goalEditBusy,
  directoryPickerOpen,
  settingsOpen,
  agentManagerOpen,
  agentManagerWorkspace,
  appVersion,
  auth,
  accountProfile,
  accountUsage,
  accountUsageBusy,
  accountUsageError,
  colorScheme,
  language,
  notificationsEnabled,
  notice,
  onCloseLogin,
  onStartLogin,
  onRefreshAccountUsage,
  onSignOut,
  onSubmitCredential,
  onCloseRemoval,
  onConfirmRemoval,
  onCloseUndoMessage,
  onConfirmUndoMessage,
  onCloseGoalEdit,
  onConfirmGoalEdit,
  onCloseDirectoryPicker,
  onSelectDirectory,
  onColorSchemeChange,
  onLanguageChange,
  onNotificationsEnabledChange,
  onProvidersChanged,
  onPluginsChanged,
  onCloseSettings,
  onCloseAgentManager,
  onDismissNotice,
}: AppOverlaysProps) {
  return (
    <>
      {loginOpen && (
        <LoginDialog
          busy={loginBusy}
          code={deviceCode}
          onClose={onCloseLogin}
          onStart={onStartLogin}
        />
      )}

      {webAuthOpen && !isDesktop() && (
        <WebCredentialDialog onSubmit={onSubmitCredential} />
      )}

      {removalTarget && (
        <RemovalDialog
          target={removalTarget}
          busy={removalBusy}
          onClose={onCloseRemoval}
          onConfirm={onConfirmRemoval}
        />
      )}

      {undoMessageTarget && (
        <UndoMessageDialog
          busy={undoMessageBusy}
          onClose={onCloseUndoMessage}
          onConfirm={onConfirmUndoMessage}
        />
      )}

      {goalEditTarget && (
        <GoalEditDialog
          goal={goalEditTarget}
          busy={goalEditBusy}
          onClose={onCloseGoalEdit}
          onConfirm={(objective) => onConfirmGoalEdit(goalEditTarget, objective)}
        />
      )}

      {directoryPickerOpen && !isDesktop() && (
        <DirectoryPickerDialog
          onClose={onCloseDirectoryPicker}
          onSelect={onSelectDirectory}
        />
      )}

      {settingsOpen && (
        <SettingsDialog
          appVersion={appVersion}
          colorScheme={colorScheme}
          language={language}
          notificationsEnabled={notificationsEnabled}
          auth={auth}
          accountProfile={accountProfile}
          accountUsage={accountUsage}
          accountUsageBusy={accountUsageBusy}
          accountUsageError={accountUsageError}
          onRefreshAccountUsage={onRefreshAccountUsage}
          onLogin={onStartLogin}
          onSignOut={onSignOut}
          onColorSchemeChange={onColorSchemeChange}
          onLanguageChange={onLanguageChange}
          onNotificationsEnabledChange={onNotificationsEnabledChange}
          onProvidersChanged={onProvidersChanged}
          onPluginsChanged={onPluginsChanged}
          onClose={onCloseSettings}
        />
      )}

      {agentManagerOpen && agentManagerWorkspace && (
        <CustomAgentManagerDialog
          workspaceId={agentManagerWorkspace.id}
          projectName={agentManagerWorkspace.name}
          onClose={onCloseAgentManager}
        />
      )}

      {notice && (
        <div className="toast" role="status">
          <span>{notice}</span>
          <button aria-label={t("notice.dismiss")} onClick={onDismissNotice}>
            <X size={14} />
          </button>
        </div>
      )}
    </>
  );
}
