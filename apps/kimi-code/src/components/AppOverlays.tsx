import { X } from "lucide-react";
import type {
  ColorScheme,
  CustomColorKey,
  CustomColors,
  CustomFonts,
  FontFamilyPreset,
  FontRole,
  FontSize,
} from "../appearance";
import type { RenderMessage } from "../chat/history";
import type { Language } from "../i18n";
import { t } from "../i18n";
import SettingsDialog from "../SettingsDialog";
import { CustomAgentManagerDialog } from "./CustomAgentManagerDialog";
import { CronTaskManagerDialog } from "./CronTaskManagerDialog";
import { isDesktop } from "../transport";
import type {
  AccountProfile,
  AccountUsage,
  AuthStatus,
  DeviceCode,
  GoalSnapshot,
  Model,
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
  cronManagerOpen: boolean;
  cronManagerSession?: { id: string };
  appVersion?: string;
  auth: AuthStatus;
  accountProfile?: AccountProfile;
  accountUsage?: AccountUsage;
  accountUsageBusy: boolean;
  accountUsageError?: string;
  colorScheme: ColorScheme;
  fontSize: FontSize;
  customColors: CustomColors;
  customFonts: CustomFonts;
  language: Language;
  notificationsEnabled: boolean;
  autoConversationTitlesEnabled: boolean;
  conversationTitleModel?: string;
  models: Model[];
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
  onFontSizeChange: (fontSize: FontSize) => void;
  onCustomColorChange: (key: CustomColorKey, value: string | undefined) => void;
  onCustomFontsChange: (
    key: FontRole,
    value: FontFamilyPreset,
  ) => void;
  onCustomFontNameChange: (role: FontRole, value: string) => void;
  onLanguageChange: (language: Language) => void;
  onNotificationsEnabledChange: (enabled: boolean) => Promise<void>;
  onAutoConversationTitlesEnabledChange: (enabled: boolean) => void;
  onConversationTitleModelChange: (modelId?: string) => void;
  onProvidersChanged: () => void;
  onPluginsChanged: () => void;
  onCloseSettings: () => void;
  onCloseAgentManager: () => void;
  onCloseCronManager: () => void;
  onCronTaskCountChange: (sessionId: string, count: number) => void;
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
  cronManagerOpen,
  cronManagerSession,
  appVersion,
  auth,
  accountProfile,
  accountUsage,
  accountUsageBusy,
  accountUsageError,
  colorScheme,
  fontSize,
  customColors,
  customFonts,
  language,
  notificationsEnabled,
  autoConversationTitlesEnabled,
  conversationTitleModel,
  models,
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
  onFontSizeChange,
  onCustomColorChange,
  onCustomFontsChange,
  onCustomFontNameChange,
  onLanguageChange,
  onNotificationsEnabledChange,
  onAutoConversationTitlesEnabledChange,
  onConversationTitleModelChange,
  onProvidersChanged,
  onPluginsChanged,
  onCloseSettings,
  onCloseAgentManager,
  onCloseCronManager,
  onCronTaskCountChange,
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

      <SettingsDialog
        open={settingsOpen}
        appVersion={appVersion}
        colorScheme={colorScheme}
        fontSize={fontSize}
        customColors={customColors}
        customFonts={customFonts}
        language={language}
        notificationsEnabled={notificationsEnabled}
        autoConversationTitlesEnabled={autoConversationTitlesEnabled}
        conversationTitleModel={conversationTitleModel}
        models={models}
        auth={auth}
        accountProfile={accountProfile}
        accountUsage={accountUsage}
        accountUsageBusy={accountUsageBusy}
        accountUsageError={accountUsageError}
        onRefreshAccountUsage={onRefreshAccountUsage}
        onLogin={onStartLogin}
        onSignOut={onSignOut}
        onColorSchemeChange={onColorSchemeChange}
        onFontSizeChange={onFontSizeChange}
        onCustomColorChange={onCustomColorChange}
        onCustomFontsChange={onCustomFontsChange}
        onCustomFontNameChange={onCustomFontNameChange}
        onLanguageChange={onLanguageChange}
        onNotificationsEnabledChange={onNotificationsEnabledChange}
        onAutoConversationTitlesEnabledChange={
          onAutoConversationTitlesEnabledChange
        }
        onConversationTitleModelChange={onConversationTitleModelChange}
        onProvidersChanged={onProvidersChanged}
        onPluginsChanged={onPluginsChanged}
        onClose={onCloseSettings}
      />

      {agentManagerOpen && agentManagerWorkspace && (
        <CustomAgentManagerDialog
          workspaceId={agentManagerWorkspace.id}
          projectName={agentManagerWorkspace.name}
          onClose={onCloseAgentManager}
        />
      )}

      {cronManagerOpen && cronManagerSession && (
        <CronTaskManagerDialog
          sessionId={cronManagerSession.id}
          onCountChange={(count) => onCronTaskCountChange(cronManagerSession.id, count)}
          onClose={onCloseCronManager}
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
