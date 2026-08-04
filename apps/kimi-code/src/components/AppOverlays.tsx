import { X } from "lucide-react";
import type { ColorScheme } from "../appearance";
import type { RenderMessage } from "../chat/history";
import type { Language } from "../i18n";
import { t } from "../i18n";
import SettingsDialog from "../SettingsDialog";
import { isDesktop } from "../transport";
import type { DeviceCode, GoalSnapshot } from "../types";
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
  appVersion?: string;
  colorScheme: ColorScheme;
  language: Language;
  notice?: string;
  onCloseLogin: () => void;
  onStartLogin: () => void;
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
  onCloseSettings: () => void;
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
  appVersion,
  colorScheme,
  language,
  notice,
  onCloseLogin,
  onStartLogin,
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
  onCloseSettings,
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
          onColorSchemeChange={onColorSchemeChange}
          onLanguageChange={onLanguageChange}
          onClose={onCloseSettings}
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
