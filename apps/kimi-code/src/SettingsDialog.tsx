import { useCallback, useEffect, useRef, useState } from "react";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { Copy, ExternalLink, Globe, Info, Puzzle, RefreshCw, SlidersHorizontal, User, X, Zap } from "lucide-react";

import type { ColorScheme } from "./appearance";
import AccountSettings from "./AccountSettings";
import SettingsSelect from "./components/SettingsSelect";
import { LANGUAGE_OPTIONS, t, type Language } from "./i18n";
import PluginSettings from "./PluginSettings";
import ProviderSettings from "./ProviderSettings";
import { invoke, isDesktop, openExternalUrl } from "./transport";
import type { AccountUsage, AuthStatus, ManagedUserInfo } from "./types";

type SettingsTab = "general" | "account" | "providers" | "plugins" | "web" | "about";
type WebServerListenScope = "local" | "global";

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

export default function SettingsDialog({
  appVersion,
  colorScheme,
  language,
  notificationsEnabled,
  auth,
  accountProfile,
  accountUsage,
  accountUsageBusy,
  accountUsageError,
  onRefreshAccountUsage,
  onLogin,
  onSignOut,
  onColorSchemeChange,
  onLanguageChange,
  onNotificationsEnabledChange,
  onProvidersChanged,
  onPluginsChanged,
  onClose,
}: {
  appVersion?: string;
  colorScheme: ColorScheme;
  language: Language;
  notificationsEnabled: boolean;
  auth: AuthStatus;
  accountProfile?: ManagedUserInfo;
  accountUsage?: AccountUsage;
  accountUsageBusy: boolean;
  accountUsageError?: string;
  onRefreshAccountUsage: () => void;
  onLogin: () => void;
  onSignOut: () => void;
  onColorSchemeChange: (colorScheme: ColorScheme) => void;
  onLanguageChange: (language: Language) => void;
  onNotificationsEnabledChange: (enabled: boolean) => Promise<void>;
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
    if (!isDesktop()) return;
    let active = true;
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
  }, []);

  useEffect(() => {
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
  }, [onClose]);

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
              className={`settings-tab ${activeTab === "account" ? "active" : ""}`}
              type="button"
              aria-current={activeTab === "account" ? "page" : undefined}
              onClick={() => setActiveTab("account")}
            >
              <User size={15} />
              {t("settings.tabAccount")}
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
            ) : activeTab === "providers" ? (
              <ProviderSettings onChanged={onProvidersChanged} />
            ) : activeTab === "plugins" ? (
              <PluginSettings onChanged={onPluginsChanged} />
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
                    className="settings-update-button"
                    type="button"
                    disabled={!webStatus?.accessUrl}
                    onClick={() => void copyProtectedWebUrl()}
                  >
                    <Copy size={14} />
                    {webCopied ? t("common.copied") : t("settings.webCopy")}
                  </button>
                </div>
              </section>
            ) : (
              <section
                className="settings-section settings-about"
                aria-labelledby="about-heading"
              >
                <h3 id="about-heading">Kimi Code</h3>
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
