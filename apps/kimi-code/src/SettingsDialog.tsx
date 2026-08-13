import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { Activity, Archive, Bot, Check, Copy, ExternalLink, Github, Globe, Info, Puzzle, RefreshCw, SlidersHorizontal, User, X, Zap } from "lucide-react";

import {
  ACCENT_COLOR_PRESETS,
  CUSTOM_FONT_NAME_MAX_LENGTH,
  DEFAULT_SCHEME_COLORS,
  FONT_SIZE_OPTIONS,
  type ColorScheme,
  type CustomColorKey,
  type CustomColors,
  type CustomFonts,
  type CodeFontPreset,
  type FontFamilyPreset,
  type FontSize,
  type FontRole,
  type InterfaceFontPreset,
} from "./appearance";
import AccountSettings from "./AccountSettings";
import AgentSettings from "./AgentSettings";
import ArchivedSessionsSettings from "./ArchivedSessionsSettings";
import SettingsSelect, {
  type SettingsSelectOption,
} from "./components/SettingsSelect";
import { LANGUAGE_OPTIONS, t, type Language } from "./i18n";
import PluginSettings from "./PluginSettings";
import ProviderSettings from "./ProviderSettings";
import { invoke, isDesktop, openExternalUrl } from "./transport";
import type { AccountProfile, AccountUsage, AuthStatus, Model } from "./types";
import UsageStatisticsSettings from "./UsageStatisticsSettings";
import type { UsageStatistics } from "./usageStatistics";

type SettingsTab = "general" | "agent" | "account" | "usage" | "providers" | "plugins" | "web" | "archived" | "about";
type WebServerListenScope = "local" | "global";
const CURRENT_CONVERSATION_MODEL = "__current_conversation_model__";

interface WebServerStatus {
  state: "stopped" | "starting" | "running" | "error";
  enabled: boolean;
  port: number;
  listenScope: WebServerListenScope;
  listenAddress: string;
  origin?: string;
  accessUrl?: string;
  error?: string;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function webServerStateLabel(state: WebServerStatus["state"] = "stopped"): string {
  switch (state) {
    case "starting":
      return t("settings.webState.starting");
    case "running":
      return t("settings.webState.running");
    case "error":
      return t("settings.webState.error");
    default:
      return t("settings.webState.stopped");
  }
}

function FontSettingRow<T extends string>({
  label,
  description,
  value,
  options,
  preview,
  mono = false,
  customValue,
  onChange,
  onCustomValueChange,
}: {
  label: string;
  description: string;
  value: T;
  options: readonly SettingsSelectOption<T>[];
  preview: string;
  mono?: boolean;
  customValue?: string;
  onChange: (value: T) => void;
  onCustomValueChange: (value: string) => void;
}) {
  const customSelected = value === "custom";
  const customAvailable = customValue ? isFontAvailable(customValue) : false;

  return (
    <div className="settings-row settings-font-row">
      <div className="settings-row-copy">
        <span className="settings-row-label">{label}</span>
        <small>{description}</small>
      </div>
      <div className="settings-font-controls">
        <span className={`settings-font-preview ${mono ? "mono" : ""}`}>
          {preview}
        </span>
        <SettingsSelect
          className="settings-font-select"
          value={value}
          options={options}
          ariaLabel={label}
          onChange={onChange}
        />
      </div>
      {customSelected ? (
        <div className="settings-custom-font">
          <input
            type="text"
            value={customValue ?? ""}
            maxLength={CUSTOM_FONT_NAME_MAX_LENGTH}
            placeholder={t("settings.fontCustomPlaceholder")}
            aria-label={t("settings.fontCustomName", { label })}
            spellCheck={false}
            onChange={(event) => onCustomValueChange(event.target.value)}
          />
          <span className={customAvailable ? "available" : "unavailable"}>
            {customValue
              ? customAvailable
                ? t("settings.fontDetected")
                : t("settings.fontNotDetected")
              : t("settings.fontCustomHint")}
          </span>
        </div>
      ) : null}
    </div>
  );
}

function AccentColorSetting({
  label,
  colorScheme,
  value,
  fallback,
  onChange,
  onReset,
}: {
  label: string;
  colorScheme: ColorScheme;
  value?: string;
  fallback: string;
  onChange: (value: string) => void;
  onReset: () => void;
}) {
  const currentColor = (value ?? fallback).toLowerCase();
  const isCustomColor = !ACCENT_COLOR_PRESETS.some(
    (preset) => preset.values[colorScheme] === currentColor,
  );

  return (
    <div className="settings-row settings-accent-setting">
      <span className="settings-row-label">{label}</span>
      <div className="settings-color-controls" role="group" aria-label={label}>
        {ACCENT_COLOR_PRESETS.map((preset) => {
          const presetValue = preset.values[colorScheme];
          const active = currentColor === presetValue;
          return (
            <button
              className={`settings-color-swatch ${active ? "active" : ""}`}
              type="button"
              key={preset.name}
              aria-label={`${label} ${preset.name}`}
              aria-pressed={active}
              style={{ "--swatch-color": presetValue } as CSSProperties}
              onClick={() => onChange(presetValue)}
            >
              <span />
            </button>
          );
        })}
        <label
          className={`settings-color-custom ${isCustomColor ? "active" : ""}`}
          title={t("settings.customColor")}
        >
          <input
            type="color"
            value={value ?? fallback}
            aria-label={t("settings.customColor")}
            onChange={(event) => onChange(event.target.value)}
          />
          <span style={{ "--swatch-color": currentColor } as CSSProperties} />
        </label>
        {value ? (
          <button
            className="settings-color-reset"
            type="button"
            onClick={onReset}
          >
            {t("settings.resetColor")}
          </button>
        ) : null}
      </div>
    </div>
  );
}

function isFontAvailable(fontFamily: string): boolean {
  if (typeof document === "undefined") return true;
  try {
    const canvas = document.createElement("canvas");
    const context = canvas.getContext("2d");
    if (!context) return true;
    const sample = "mmmmmmmmmmWWWWW月亮0123456789@#";
    const fallbacks = ["monospace", "serif", "sans-serif"];
    return fallbacks.some((fallback) => {
      context.font = `72px ${fallback}`;
      const fallbackWidth = context.measureText(sample).width;
      context.font = `72px ${JSON.stringify(fontFamily)}, ${fallback}`;
      return context.measureText(sample).width !== fallbackWidth;
    });
  } catch {
    return false;
  }
}

function availabilityDescription(
  description: string,
  available: boolean,
): string {
  return `${description} · ${t(
    available ? "settings.fontAvailable" : "settings.fontUnavailable",
  )}`;
}

function interfaceFontOptions(): readonly SettingsSelectOption<InterfaceFontPreset>[] {
  const notoAvailable = isFontAvailable("Noto Sans SC");
  const yaheiAvailable = isFontAvailable("Microsoft YaHei");
  const pingfangAvailable = isFontAvailable("PingFang SC");
  return [
    {
      value: "kimi",
      label: t("settings.fontInterfaceKimi"),
      description: t("settings.fontInterfaceKimiDescription"),
    },
    {
      value: "system",
      label: t("settings.fontInterfaceSystem"),
      description: t("settings.fontInterfaceSystemDescription"),
    },
    {
      value: "noto-sans",
      label: t("settings.fontInterfaceNotoSans"),
      description: availabilityDescription(
        t("settings.fontInterfaceNotoSansDescription"),
        notoAvailable,
      ),
      disabled: !notoAvailable,
    },
    {
      value: "microsoft-yahei",
      label: "Microsoft YaHei",
      description: availabilityDescription(
        t("settings.fontInterfaceYaheiDescription"),
        yaheiAvailable,
      ),
      disabled: !yaheiAvailable,
    },
    {
      value: "pingfang",
      label: "PingFang SC",
      description: availabilityDescription(
        t("settings.fontInterfacePingfangDescription"),
        pingfangAvailable,
      ),
      disabled: !pingfangAvailable,
    },
    {
      value: "serif",
      label: t("settings.fontInterfaceSerif"),
      description: t("settings.fontInterfaceSerifDescription"),
    },
    {
      value: "custom",
      label: t("settings.fontCustom"),
      description: t("settings.fontCustomDescription"),
    },
  ];
}

function codeFontOptions(): readonly SettingsSelectOption<CodeFontPreset>[] {
  const cascadiaAvailable =
    isFontAvailable("Cascadia Code") || isFontAvailable("Cascadia Mono");
  const consolasAvailable = isFontAvailable("Consolas");
  const sfMonoAvailable = isFontAvailable("SF Mono");
  const menloAvailable = isFontAvailable("Menlo");
  return [
    {
      value: "kimi",
      label: t("settings.fontCodeKimi"),
      description: t("settings.fontCodeKimiDescription"),
    },
    {
      value: "system",
      label: t("settings.fontCodeSystem"),
      description: t("settings.fontCodeSystemDescription"),
    },
    {
      value: "cascadia",
      label: "Cascadia Code",
      description: availabilityDescription(
        t("settings.fontCodeCascadiaDescription"),
        cascadiaAvailable,
      ),
      disabled: !cascadiaAvailable,
    },
    {
      value: "consolas",
      label: "Consolas",
      description: availabilityDescription(
        t("settings.fontCodeConsolasDescription"),
        consolasAvailable,
      ),
      disabled: !consolasAvailable,
    },
    {
      value: "sf-mono",
      label: "SF Mono",
      description: availabilityDescription(
        t("settings.fontCodeSfMonoDescription"),
        sfMonoAvailable,
      ),
      disabled: !sfMonoAvailable,
    },
    {
      value: "menlo",
      label: "Menlo",
      description: availabilityDescription(
        t("settings.fontCodeMenloDescription"),
        menloAvailable,
      ),
      disabled: !menloAvailable,
    },
    {
      value: "custom",
      label: t("settings.fontCustom"),
      description: t("settings.fontCustomDescription"),
    },
  ];
}

function conversationTitleModelOptions(
  models: readonly Model[],
  selected?: string,
): readonly SettingsSelectOption<string>[] {
  const options: SettingsSelectOption<string>[] = [
    {
      value: CURRENT_CONVERSATION_MODEL,
      label: t("settings.conversationTitleModelCurrent"),
      description: t("settings.conversationTitleModelCurrentDescription"),
    },
    ...models.map((model) => ({
      value: model.id,
      label: model.displayName,
      description: `${model.providerId} · ${model.model}`,
    })),
  ];
  if (selected && !models.some((model) => model.id === selected)) {
    options.push({
      value: selected,
      label: selected,
      description: t("settings.conversationTitleModelUnavailable"),
      disabled: true,
    });
  }
  return options;
}

export default function SettingsDialog({
  open,
  appVersion,
  colorScheme,
  fontSize,
  customColors,
  customFonts,
  language,
  notificationsEnabled,
  autoConversationTitlesEnabled,
  conversationTitleModel,
  models,
  auth,
  accountProfile,
  accountUsage,
  accountUsageBusy,
  accountUsageError,
  onRefreshAccountUsage,
  onLogin,
  onSignOut,
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
  onClose,
}: {
  open: boolean;
  appVersion?: string;
  colorScheme: ColorScheme;
  fontSize: FontSize;
  customColors: CustomColors;
  customFonts: CustomFonts;
  language: Language;
  notificationsEnabled: boolean;
  autoConversationTitlesEnabled: boolean;
  conversationTitleModel?: string;
  models: Model[];
  auth: AuthStatus;
  accountProfile?: AccountProfile;
  accountUsage?: AccountUsage;
  accountUsageBusy: boolean;
  accountUsageError?: string;
  onRefreshAccountUsage: () => void;
  onLogin: () => void;
  onSignOut: () => void;
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
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const [activeTab, setActiveTab] = useState<SettingsTab>("general");
  const [pendingUpdate, setPendingUpdate] = useState<Update | null>(null);
  const [updateBusy, setUpdateBusy] = useState(false);
  const [updateMessage, setUpdateMessage] = useState<string>();
  const [updateToast, setUpdateToast] = useState<string>();
  const [downloadProgress, setDownloadProgress] = useState<number>();
  const [webStatus, setWebStatus] = useState<WebServerStatus>();
  const [webEnabled, setWebEnabled] = useState(false);
  const [webPort, setWebPort] = useState("58627");
  const [webListenScope, setWebListenScope] =
    useState<WebServerListenScope>("local");
  const [webBusy, setWebBusy] = useState(true);
  const [webError, setWebError] = useState<string>();
  const [webCopied, setWebCopied] = useState(false);
  const [notificationBusy, setNotificationBusy] = useState(false);
  const [usageStatistics, setUsageStatistics] = useState<UsageStatistics>();
  const [usageStatisticsBusy, setUsageStatisticsBusy] = useState(false);
  const [usageStatisticsError, setUsageStatisticsError] = useState<string>();
  const usageStatisticsBusyRef = useRef(false);
  const updateToastTimerRef = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined,
  );

  const clearUpdateToastTimer = useCallback((): void => {
    if (updateToastTimerRef.current === undefined) return;
    clearTimeout(updateToastTimerRef.current);
    updateToastTimerRef.current = undefined;
  }, []);

  const hideUpdateToast = useCallback((): void => {
    clearUpdateToastTimer();
    setUpdateToast(undefined);
  }, [clearUpdateToastTimer]);

  const scheduleUpdateToastDismiss = useCallback((): void => {
    clearUpdateToastTimer();
    updateToastTimerRef.current = setTimeout(() => {
      updateToastTimerRef.current = undefined;
      setUpdateToast(undefined);
    }, 3_000);
  }, [clearUpdateToastTimer]);

  const showUpdateToast = useCallback(
    (message: string): void => {
      setUpdateToast(message);
      scheduleUpdateToastDismiss();
    },
    [scheduleUpdateToastDismiss],
  );

  useEffect(
    () => () => {
      clearUpdateToastTimer();
    },
    [clearUpdateToastTimer],
  );

  useEffect(() => {
    if (!open || !isDesktop()) return;
    let active = true;
    setWebBusy(true);
    void invoke<WebServerStatus>("get_web_server_status")
      .then((status) => {
        if (!active) return;
        setWebStatus(status);
        setWebEnabled(status.enabled);
        setWebPort(String(status.port));
        setWebListenScope(status.listenScope);
        setWebError(status.error);
      })
      .catch((error) => {
        if (active) setWebError(errorMessage(error));
      })
      .finally(() => {
        if (active) setWebBusy(false);
      });
    return () => {
      active = false;
    };
  }, [open]);

  const refreshUsageStatistics = useCallback(async (): Promise<void> => {
    if (usageStatisticsBusyRef.current) return;
    usageStatisticsBusyRef.current = true;
    setUsageStatisticsBusy(true);
    setUsageStatisticsError(undefined);
    try {
      setUsageStatistics(
        await invoke<UsageStatistics>("get_usage_statistics"),
      );
    } catch (error) {
      setUsageStatisticsError(errorMessage(error));
    } finally {
      usageStatisticsBusyRef.current = false;
      setUsageStatisticsBusy(false);
    }
  }, []);

  useEffect(() => {
    if (open && activeTab === "usage") {
      void refreshUsageStatistics();
    }
  }, [activeTab, open, refreshUsageStatistics]);

  useEffect(() => {
    if (!open) return;
    const previousFocus = document.activeElement;
    dialogRef.current?.focus();

    const handleKeyDown = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }

      if (event.key !== "Tab" || !dialogRef.current) return;
      const focusableElements = Array.from(
        dialogRef.current.querySelectorAll<HTMLElement>(
          'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ),
      );
      if (focusableElements.length === 0) {
        event.preventDefault();
        dialogRef.current.focus();
        return;
      }

      const firstElement = focusableElements[0];
      const lastElement = focusableElements[focusableElements.length - 1];
      if (event.shiftKey && document.activeElement === firstElement) {
        event.preventDefault();
        lastElement.focus();
      } else if (!event.shiftKey && document.activeElement === lastElement) {
        event.preventDefault();
        firstElement.focus();
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      if (previousFocus instanceof HTMLElement) previousFocus.focus();
    };
  }, [onClose, open]);

  const handleCheckForUpdates = async (): Promise<void> => {
    if (!isDesktop()) {
      showUpdateToast(t("settings.updateDesktopOnly"));
      return;
    }

    setUpdateBusy(true);
    hideUpdateToast();
    setUpdateMessage(undefined);
    setPendingUpdate(null);
    setDownloadProgress(undefined);
    try {
      const update = await check({ timeout: 30_000 });
      if (!update) {
        showUpdateToast(t("settings.updateLatest"));
        return;
      }

      setPendingUpdate(update);
      setUpdateMessage(
        t("settings.updateFound", { version: update.version }),
      );
    } catch (error) {
      showUpdateToast(
        t("settings.updateCheckFailed", { error: errorMessage(error) }),
      );
    } finally {
      setUpdateBusy(false);
    }
  };

  const toggleWebServer = async (): Promise<void> => {
    if (webBusy) return;
    const enabled = !webEnabled;
    const port = Number(webPort);
    if (enabled && (!Number.isInteger(port) || port < 1 || port > 65_535)) {
      setWebError(t("settings.webPortInvalid"));
      return;
    }
    const previousEnabled = webEnabled;
    setWebBusy(true);
    setWebEnabled(enabled);
    setWebError(undefined);
    try {
      const status = await invoke<WebServerStatus>("set_web_server_settings", {
        settings: { enabled, port, listenScope: webListenScope },
      });
      setWebStatus(status);
      setWebEnabled(status.enabled);
      setWebPort(String(status.port));
      setWebListenScope(status.listenScope);
      setWebError(status.error);
    } catch (error) {
      setWebEnabled(previousEnabled);
      setWebError(errorMessage(error));
      try {
        const status = await invoke<WebServerStatus>("get_web_server_status");
        setWebStatus(status);
        setWebEnabled(status.enabled);
        if (status.enabled) {
          setWebPort(String(status.port));
          setWebListenScope(status.listenScope);
        }
      } catch {
        // Preserve the actionable settings error.
      }
    } finally {
      setWebBusy(false);
    }
  };

  const webConfigurationLocked = webEnabled || webBusy;

  const copyProtectedWebUrl = async (): Promise<void> => {
    if (!webStatus?.accessUrl) return;
    await navigator.clipboard.writeText(webStatus.accessUrl);
    setWebCopied(true);
    window.setTimeout(() => setWebCopied(false), 1500);
  };

  const handleInstallUpdate = async (): Promise<void> => {
    if (!pendingUpdate) return;

    setUpdateBusy(true);
    hideUpdateToast();
    setUpdateMessage(
      t("settings.updateDownloading", { version: pendingUpdate.version }),
    );
    setDownloadProgress(0);
    let downloaded = 0;
    let contentLength = 0;

    try {
      await pendingUpdate.downloadAndInstall((event) => {
        if (event.event === "Started") {
          contentLength = event.data.contentLength ?? 0;
        } else if (event.event === "Progress") {
          downloaded += event.data.chunkLength;
          if (contentLength > 0) {
            setDownloadProgress(
              Math.min(100, Math.round((downloaded / contentLength) * 100)),
            );
          }
        } else if (event.event === "Finished") {
          setDownloadProgress(100);
          setUpdateMessage(t("settings.updateInstalled"));
        }
      });
      await relaunch();
    } catch (error) {
      showUpdateToast(
        t("settings.updateInstallFailed", { error: errorMessage(error) }),
      );
      setUpdateMessage(
        t("settings.updateFound", { version: pendingUpdate.version }),
      );
      setDownloadProgress(undefined);
      setUpdateBusy(false);
    }
  };

  const toggleNotifications = async (): Promise<void> => {
    if (notificationBusy || !isDesktop()) return;
    setNotificationBusy(true);
    try {
      await onNotificationsEnabledChange(!notificationsEnabled);
    } finally {
      setNotificationBusy(false);
    }
  };

  // Keep this component mounted while hidden so an in-flight updater request
  // retains its progress and pending Update resource across dialog reopenings.
  if (!open) return null;

  return (
    <div
      className="settings-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="settings-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-dialog-title"
        tabIndex={-1}
      >
        <div className="settings-dialog-layout">
          <nav className="settings-tabs" aria-label={t("settings.tabs")}>
            <div className="settings-tabs-header">
              <h2 id="settings-dialog-title">{t("settings.title")}</h2>
            </div>
            <button
              className={`settings-tab ${activeTab === "general" ? "active" : ""}`}
              type="button"
              aria-current={activeTab === "general" ? "page" : undefined}
              onClick={() => setActiveTab("general")}
            >
              <SlidersHorizontal size={15} />
              {t("settings.tabGeneral")}
            </button>
            <button
              className={`settings-tab ${activeTab === "agent" ? "active" : ""}`}
              type="button"
              aria-current={activeTab === "agent" ? "page" : undefined}
              onClick={() => setActiveTab("agent")}
            >
              <Bot size={15} />
              {t("settings.tabAgent")}
            </button>
            <button
              className={`settings-tab ${activeTab === "account" ? "active" : ""}`}
              type="button"
              aria-current={activeTab === "account" ? "page" : undefined}
              onClick={() => setActiveTab("account")}
            >
              <User size={15} />
              {t("settings.tabAccount")}
            </button>
            <button
              className={`settings-tab ${activeTab === "usage" ? "active" : ""}`}
              type="button"
              aria-current={activeTab === "usage" ? "page" : undefined}
              onClick={() => setActiveTab("usage")}
            >
              <Activity size={15} />
              {t("settings.tabUsage")}
            </button>
            <button
              className={`settings-tab ${activeTab === "providers" ? "active" : ""}`}
              type="button"
              aria-current={activeTab === "providers" ? "page" : undefined}
              onClick={() => setActiveTab("providers")}
            >
              <Zap size={15} />
              {t("settings.tabProviders")}
            </button>
            <button
              className={`settings-tab ${activeTab === "plugins" ? "active" : ""}`}
              type="button"
              aria-current={activeTab === "plugins" ? "page" : undefined}
              onClick={() => setActiveTab("plugins")}
            >
              <Puzzle size={15} />
              {t("settings.tabPlugins")}
            </button>
            {isDesktop() && (
              <button
                className={`settings-tab ${activeTab === "web" ? "active" : ""}`}
                type="button"
                aria-current={activeTab === "web" ? "page" : undefined}
                onClick={() => setActiveTab("web")}
              >
                <Globe size={15} />
                {t("settings.tabWeb")}
              </button>
            )}
            <button
              className={`settings-tab ${activeTab === "archived" ? "active" : ""}`}
              type="button"
              aria-current={activeTab === "archived" ? "page" : undefined}
              onClick={() => setActiveTab("archived")}
            >
              <Archive size={15} />
              {t("settings.tabArchived")}
            </button>
            <button
              className={`settings-tab ${activeTab === "about" ? "active" : ""}`}
              type="button"
              aria-current={activeTab === "about" ? "page" : undefined}
              onClick={() => setActiveTab("about")}
            >
              <Info size={15} />
              {t("settings.tabAbout")}
            </button>
          </nav>

          <div className="settings-region">
            <div className="settings-region-header">
              <button
                className="settings-dialog-close"
                type="button"
                aria-label={t("settings.close")}
                onClick={onClose}
              >
                <X size={16} />
              </button>
            </div>

            <main className="settings-dialog-content">
            {activeTab === "general" ? (
              <>
                <section
                  className="settings-section"
                  aria-labelledby="appearance-heading"
                >
                  <h3 id="appearance-heading">{t("settings.appearance")}</h3>
                  <div className="settings-row">
                    <span className="settings-row-label">
                      {t("settings.theme")}
                    </span>
                    <div
                      className="settings-segmented"
                      role="group"
                      aria-label={t("settings.themeGroup")}
                    >
                      <button
                        className={colorScheme === "light" ? "active" : ""}
                        type="button"
                        aria-pressed={colorScheme === "light"}
                        onClick={() => onColorSchemeChange("light")}
                      >
                        {t("settings.themeLight")}
                      </button>
                      <button
                        className={colorScheme === "dark" ? "active" : ""}
                        type="button"
                        aria-pressed={colorScheme === "dark"}
                        onClick={() => onColorSchemeChange("dark")}
                      >
                        {t("settings.themeDark")}
                      </button>
                    </div>
                  </div>
                  <div className="settings-row">
                    <span className="settings-row-label">
                      {t("settings.language")}
                    </span>
                    <SettingsSelect
                      value={language}
                      options={LANGUAGE_OPTIONS}
                      ariaLabel={t("settings.language")}
                      onChange={onLanguageChange}
                    />
                  </div>
                  <div className="settings-subsection-heading">
                    <strong>{t("settings.typography")}</strong>
                    <span>{t("settings.typographyDescription")}</span>
                  </div>
                  <div className="settings-row settings-font-row">
                    <div className="settings-row-copy">
                      <span className="settings-row-label">
                        {t("settings.fontSize")}
                      </span>
                      <small>{t("settings.fontSizeDescription")}</small>
                    </div>
                    <SettingsSelect
                      value={fontSize}
                      options={FONT_SIZE_OPTIONS}
                      ariaLabel={t("settings.fontSize")}
                      onChange={onFontSizeChange}
                    />
                  </div>
                  <FontSettingRow
                    label={t("settings.fontInterface")}
                    description={t("settings.fontInterfaceDescription")}
                    value={customFonts.sans ?? "kimi"}
                    options={interfaceFontOptions()}
                    preview={t("settings.fontInterfacePreview")}
                    customValue={customFonts.sansCustom}
                    onChange={(value) => onCustomFontsChange("sans", value)}
                    onCustomValueChange={(value) =>
                      onCustomFontNameChange("sans", value)
                    }
                  />
                  <FontSettingRow
                    label={t("settings.fontCode")}
                    description={t("settings.fontCodeDescription")}
                    value={customFonts.mono ?? "kimi"}
                    options={codeFontOptions()}
                    preview={t("settings.fontCodePreview")}
                    mono
                    customValue={customFonts.monoCustom}
                    onChange={(value) => onCustomFontsChange("mono", value)}
                    onCustomValueChange={(value) =>
                      onCustomFontNameChange("mono", value)
                    }
                  />
                  <p className="settings-section-copy settings-color-copy">
                    {t("settings.customColorsHint")}
                  </p>
                  <AccentColorSetting
                    label={t("settings.accentColor")}
                    colorScheme={colorScheme}
                    value={customColors.accent}
                    fallback={DEFAULT_SCHEME_COLORS[colorScheme].accent}
                    onChange={(value) => onCustomColorChange("accent", value)}
                    onReset={() => onCustomColorChange("accent", undefined)}
                  />
                </section>

                <section
                  className="settings-section"
                  aria-labelledby="conversation-titles-heading"
                >
                  <h3 id="conversation-titles-heading">
                    {t("settings.conversationTitles")}
                  </h3>
                  <p className="settings-section-copy">
                    {t("settings.conversationTitlesDescription")}
                  </p>
                  <div className="settings-row">
                    <div>
                      <span className="settings-row-label">
                        {t("settings.conversationTitleModel")}
                      </span>
                      <small>{t("settings.conversationTitleModelHint")}</small>
                    </div>
                    <SettingsSelect
                      value={
                        conversationTitleModel ?? CURRENT_CONVERSATION_MODEL
                      }
                      options={conversationTitleModelOptions(
                        models,
                        conversationTitleModel,
                      )}
                      ariaLabel={t("settings.conversationTitleModel")}
                      onChange={(value) =>
                        onConversationTitleModelChange(
                          value === CURRENT_CONVERSATION_MODEL
                            ? undefined
                            : value,
                        )
                      }
                    />
                  </div>
                  <div className="settings-row">
                    <div>
                      <span className="settings-row-label">
                        {t("settings.autoConversationTitles")}
                      </span>
                      <small>{t("settings.autoConversationTitlesHint")}</small>
                    </div>
                    <button
                      className={`settings-toggle ${autoConversationTitlesEnabled ? "active" : ""}`}
                      type="button"
                      role="switch"
                      aria-label={t("settings.autoConversationTitles")}
                      aria-checked={autoConversationTitlesEnabled}
                      onClick={() =>
                        onAutoConversationTitlesEnabledChange(
                          !autoConversationTitlesEnabled,
                        )
                      }
                    >
                      <span />
                    </button>
                  </div>
                </section>

                <section
                  className="settings-section"
                  aria-labelledby="notifications-heading"
                >
                  <h3 id="notifications-heading">
                    {t("settings.notifications")}
                  </h3>
                  <p className="settings-section-copy">
                    {isDesktop()
                      ? t("settings.notificationsDescription")
                      : t("settings.notificationsDesktopOnly")}
                  </p>
                  <div className="settings-row">
                    <div>
                      <span className="settings-row-label">
                        {t("settings.notificationsEnabled")}
                      </span>
                      <small>{t("settings.notificationsEvents")}</small>
                    </div>
                    <button
                      className={`settings-toggle ${isDesktop() && notificationsEnabled ? "active" : ""}`}
                      type="button"
                      role="switch"
                      aria-label={t("settings.notificationsEnabled")}
                      aria-checked={isDesktop() && notificationsEnabled}
                      disabled={!isDesktop() || notificationBusy}
                      onClick={() => void toggleNotifications()}
                    >
                      <span />
                    </button>
                  </div>
                </section>
              </>
            ) : activeTab === "agent" ? (
              <AgentSettings />
            ) : activeTab === "account" ? (
              <AccountSettings
                auth={auth}
                profile={accountProfile}
                usage={accountUsage}
                usageBusy={accountUsageBusy}
                usageError={accountUsageError}
                onRefreshUsage={onRefreshAccountUsage}
                onLogin={onLogin}
                onSignOut={onSignOut}
              />
            ) : activeTab === "usage" ? (
              <UsageStatisticsSettings
                statistics={usageStatistics}
                busy={usageStatisticsBusy}
                error={usageStatisticsError}
                language={language}
                onRetry={() => void refreshUsageStatistics()}
              />
            ) : activeTab === "providers" ? (
              <ProviderSettings onChanged={onProvidersChanged} />
            ) : activeTab === "plugins" ? (
              <PluginSettings onChanged={onPluginsChanged} />
            ) : activeTab === "archived" ? (
              <ArchivedSessionsSettings />
            ) : activeTab === "web" ? (
              <section
                className="settings-section settings-web"
                aria-labelledby="web-heading"
              >
                <h3 id="web-heading">{t("settings.webTitle")}</h3>
                <p className="settings-section-copy">
                  {t("settings.webDescription")}
                </p>
                <div className="settings-row">
                  <div>
                    <span className="settings-row-label">
                      {t("settings.webEnabled")}
                    </span>
                    <small>
                      {webEnabled
                        ? t("settings.webConfigurationLocked")
                        : webListenScope === "global"
                          ? t("settings.webGlobalDescription")
                          : t("settings.webLocalDescription")}
                    </small>
                  </div>
                  <button
                    className={`settings-toggle ${webEnabled ? "active" : ""}`}
                    type="button"
                    role="switch"
                    aria-checked={webEnabled}
                    disabled={webBusy}
                    onClick={() => void toggleWebServer()}
                  >
                    <span />
                  </button>
                </div>
                <div className="settings-row">
                  <label className="settings-row-label" htmlFor="web-server-port">
                    {t("settings.webPort")}
                  </label>
                  <input
                    id="web-server-port"
                    className="settings-port-input"
                    type="number"
                    min={1}
                    max={65535}
                    value={webPort}
                    disabled={webConfigurationLocked}
                    onChange={(event) => setWebPort(event.target.value)}
                  />
                </div>
                <div className="settings-row">
                  <span className="settings-row-label">
                    {t("settings.webListenScope")}
                  </span>
                  <SettingsSelect<WebServerListenScope>
                    className="settings-scope-select"
                    value={webListenScope}
                    options={[
                      { value: "local", label: t("settings.webScopeLocal") },
                      { value: "global", label: t("settings.webScopeGlobal") },
                    ]}
                    ariaLabel={t("settings.webListenScope")}
                    disabled={webConfigurationLocked}
                    onChange={setWebListenScope}
                  />
                </div>
                <div className="settings-web-status">
                  <span className={`state ${webStatus?.state ?? "stopped"}`} />
                  <div>
                    <strong>
                      {webServerStateLabel(webStatus?.state)}
                    </strong>
                    <small>
                      {webStatus?.origin
                        ? `${webStatus.origin} · ${webStatus.listenAddress}:${webStatus.port}`
                        : t("settings.webUnavailable")}
                    </small>
                  </div>
                </div>
                {(webError || webStatus?.error) && (
                  <div className="settings-web-error" role="alert">
                    {webError ?? webStatus?.error}
                  </div>
                )}
                <div className="settings-web-actions">
                  <button
                    className="settings-update-button"
                    type="button"
                    disabled={!webStatus?.accessUrl}
                    onClick={() => webStatus?.accessUrl && void openExternalUrl(webStatus.accessUrl)}
                  >
                    <ExternalLink size={14} />
                    {t("settings.webOpen")}
                  </button>
                  <button
                    className="settings-update-button settings-copy-button"
                    type="button"
                    title={webCopied ? t("common.copied") : t("settings.webCopy")}
                    aria-label={webCopied ? t("common.copied") : t("settings.webCopy")}
                    disabled={!webStatus?.accessUrl}
                    onClick={() => void copyProtectedWebUrl()}
                  >
                    {webCopied ? <Check size={14} /> : <Copy size={14} />}
                  </button>
                </div>
              </section>
            ) : (
              <section
                className="settings-section settings-about"
                aria-labelledby="about-heading"
              >
                <h3 id="about-heading">Kimi Code</h3>
                <div className="settings-community-card">
                  <div>
                    <strong>{t("settings.communityEdition")}</strong>
                    <p>{t("settings.communityDescription")}</p>
                  </div>
                  <button
                    className="settings-repository-link"
                    type="button"
                    aria-label={t("settings.openRepository")}
                    onClick={() =>
                      void openExternalUrl(
                        "https://github.com/vnt-dev/kimi-code-rs",
                      )
                    }
                  >
                    <Github size={15} />
                    <span>github.com/vnt-dev/kimi-code-rs</span>
                    <ExternalLink size={13} />
                  </button>
                </div>
                <div className="settings-row">
                  <div className="settings-version">
                    <span className="settings-row-label">
                      {t("settings.version")}
                    </span>
                    <span className="settings-version-number">
                      v{appVersion ?? "—"}
                    </span>
                  </div>
                  {isDesktop() ? (
                    <button
                      className="settings-update-button"
                      type="button"
                      disabled={updateBusy}
                      onClick={() =>
                        void (pendingUpdate
                          ? handleInstallUpdate()
                          : handleCheckForUpdates())
                      }
                    >
                      <RefreshCw
                        size={14}
                        className={updateBusy ? "spinning" : undefined}
                      />
                      {pendingUpdate
                        ? t("settings.installUpdate")
                        : t("settings.checkUpdate")}
                    </button>
                  ) : (
                    <span className="settings-desktop-only">
                      {t("settings.updateDesktopOnly")}
                    </span>
                  )}
                </div>

                {updateMessage && (
                  <div className="settings-update-status" role="status">
                    <span>{updateMessage}</span>
                    {downloadProgress !== undefined && (
                      <div
                        className="settings-update-progress"
                        role="progressbar"
                        aria-label={t("settings.updateProgress")}
                        aria-valuemin={0}
                        aria-valuemax={100}
                        aria-valuenow={downloadProgress}
                      >
                        <span style={{ width: `${downloadProgress}%` }} />
                      </div>
                    )}
                    {pendingUpdate?.body && !updateBusy && (
                      <p>{pendingUpdate.body}</p>
                    )}
                  </div>
                )}
              </section>
            )}
            </main>
          </div>
        </div>

        {updateToast && (
          <div
            className="settings-toast"
            role="status"
            onMouseEnter={clearUpdateToastTimer}
            onMouseLeave={scheduleUpdateToastDismiss}
          >
            <span>{updateToast}</span>
            <button
              type="button"
              aria-label={t("settings.updateToastClose")}
              onClick={hideUpdateToast}
            >
              <X size={14} />
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
