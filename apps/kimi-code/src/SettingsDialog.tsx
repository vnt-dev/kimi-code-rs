import { useCallback, useEffect, useRef, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { Check, ChevronDown, RefreshCw, X } from "lucide-react";

import type { ColorScheme } from "./appearance";
import { LANGUAGE_OPTIONS, t, type Language } from "./i18n";

type SettingsTab = "general" | "about";

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function LanguageSelect({
  language,
  onLanguageChange,
}: {
  language: Language;
  onLanguageChange: (language: Language) => void;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent): void => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, [open]);

  const activeLabel =
    LANGUAGE_OPTIONS.find((option) => option.value === language)?.label ??
    language;

  return (
    <div
      className={`settings-select ${open ? "open" : ""}`}
      ref={rootRef}
      onKeyDown={(event) => {
        if (event.key === "Escape" && open) {
          event.stopPropagation();
          setOpen(false);
        }
      }}
    >
      <button
        className="settings-select-trigger"
        type="button"
        aria-label={t("settings.language")}
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        <span>{activeLabel}</span>
        <ChevronDown size={14} />
      </button>
      {open && (
        <div
          className="settings-select-menu"
          role="listbox"
          aria-label={t("settings.language")}
        >
          {LANGUAGE_OPTIONS.map((option) => {
            const selected = option.value === language;
            return (
              <button
                key={option.value}
                type="button"
                role="option"
                aria-selected={selected}
                onClick={() => {
                  onLanguageChange(option.value);
                  setOpen(false);
                }}
              >
                <span>{option.label}</span>
                {selected && <Check size={14} />}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

export default function SettingsDialog({
  appVersion,
  colorScheme,
  language,
  onColorSchemeChange,
  onLanguageChange,
  onClose,
}: {
  appVersion?: string;
  colorScheme: ColorScheme;
  language: Language;
  onColorSchemeChange: (colorScheme: ColorScheme) => void;
  onLanguageChange: (language: Language) => void;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const [activeTab, setActiveTab] = useState<SettingsTab>("general");
  const [pendingUpdate, setPendingUpdate] = useState<Update | null>(null);
  const [updateBusy, setUpdateBusy] = useState(false);
  const [updateMessage, setUpdateMessage] = useState<string>();
  const [updateToast, setUpdateToast] = useState<string>();
  const [downloadProgress, setDownloadProgress] = useState<number>();
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
    if (!isTauri()) {
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
        <header className="settings-dialog-header">
          <h2 id="settings-dialog-title">{t("settings.title")}</h2>
          <button
            className="settings-dialog-close"
            type="button"
            aria-label={t("settings.close")}
            onClick={onClose}
          >
            <X size={17} />
          </button>
        </header>

        <div className="settings-dialog-layout">
          <nav className="settings-tabs" aria-label={t("settings.tabs")}>
            <button
              className={`settings-tab ${activeTab === "general" ? "active" : ""}`}
              type="button"
              aria-current={activeTab === "general" ? "page" : undefined}
              onClick={() => setActiveTab("general")}
            >
              {t("settings.tabGeneral")}
            </button>
            <button
              className={`settings-tab ${activeTab === "about" ? "active" : ""}`}
              type="button"
              aria-current={activeTab === "about" ? "page" : undefined}
              onClick={() => setActiveTab("about")}
            >
              {t("settings.tabAbout")}
            </button>
          </nav>

          <main className="settings-dialog-content">
            {activeTab === "general" ? (
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
                  <LanguageSelect
                    language={language}
                    onLanguageChange={onLanguageChange}
                  />
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
