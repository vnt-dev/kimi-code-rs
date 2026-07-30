import { useEffect, useRef, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { RefreshCw, X } from "lucide-react";

import type { ColorScheme } from "./appearance";

type SettingsTab = "general" | "about";

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export default function SettingsDialog({
  appVersion,
  colorScheme,
  onColorSchemeChange,
  onClose,
}: {
  appVersion?: string;
  colorScheme: ColorScheme;
  onColorSchemeChange: (colorScheme: ColorScheme) => void;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const [activeTab, setActiveTab] = useState<SettingsTab>("general");
  const [pendingUpdate, setPendingUpdate] = useState<Update | null>(null);
  const [updateBusy, setUpdateBusy] = useState(false);
  const [updateMessage, setUpdateMessage] = useState<string>();
  const [updateToast, setUpdateToast] = useState<string>();
  const [downloadProgress, setDownloadProgress] = useState<number>();

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
      setUpdateToast("请在桌面客户端中检查更新");
      return;
    }

    setUpdateBusy(true);
    setUpdateToast(undefined);
    setUpdateMessage(undefined);
    setPendingUpdate(null);
    setDownloadProgress(undefined);
    try {
      const update = await check({ timeout: 30_000 });
      if (!update) {
        setUpdateToast("当前已是最新版本");
        return;
      }

      setPendingUpdate(update);
      setUpdateMessage(`发现新版本 v${update.version}`);
    } catch (error) {
      setUpdateToast(`检查更新失败：${errorMessage(error)}`);
    } finally {
      setUpdateBusy(false);
    }
  };

  const handleInstallUpdate = async (): Promise<void> => {
    if (!pendingUpdate) return;

    setUpdateBusy(true);
    setUpdateToast(undefined);
    setUpdateMessage(`正在下载 v${pendingUpdate.version}…`);
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
          setUpdateMessage("更新已安装，正在重启…");
        }
      });
      await relaunch();
    } catch (error) {
      setUpdateToast(`安装更新失败：${errorMessage(error)}`);
      setUpdateMessage(`发现新版本 v${pendingUpdate.version}`);
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
          <h2 id="settings-dialog-title">设置</h2>
          <button
            className="settings-dialog-close"
            type="button"
            aria-label="关闭设置"
            onClick={onClose}
          >
            <X size={17} />
          </button>
        </header>

        <div className="settings-dialog-layout">
          <nav className="settings-tabs" aria-label="设置分类">
            <button
              className={`settings-tab ${activeTab === "general" ? "active" : ""}`}
              type="button"
              aria-current={activeTab === "general" ? "page" : undefined}
              onClick={() => setActiveTab("general")}
            >
              通用
            </button>
            <button
              className={`settings-tab ${activeTab === "about" ? "active" : ""}`}
              type="button"
              aria-current={activeTab === "about" ? "page" : undefined}
              onClick={() => setActiveTab("about")}
            >
              关于
            </button>
          </nav>

          <main className="settings-dialog-content">
            {activeTab === "general" ? (
              <section
                className="settings-section"
                aria-labelledby="appearance-heading"
              >
                <h3 id="appearance-heading">外观</h3>
                <div className="settings-row">
                  <span className="settings-row-label">明暗</span>
                  <div
                    className="settings-segmented"
                    role="group"
                    aria-label="明暗主题"
                  >
                    <button
                      className={colorScheme === "light" ? "active" : ""}
                      type="button"
                      aria-pressed={colorScheme === "light"}
                      onClick={() => onColorSchemeChange("light")}
                    >
                      月之亮面
                    </button>
                    <button
                      className={colorScheme === "dark" ? "active" : ""}
                      type="button"
                      aria-pressed={colorScheme === "dark"}
                      onClick={() => onColorSchemeChange("dark")}
                    >
                      月之暗面
                    </button>
                  </div>
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
                    <span className="settings-row-label">版本</span>
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
                    {pendingUpdate ? "下载并安装" : "检查更新"}
                  </button>
                </div>

                {updateMessage && (
                  <div className="settings-update-status" role="status">
                    <span>{updateMessage}</span>
                    {downloadProgress !== undefined && (
                      <div
                        className="settings-update-progress"
                        role="progressbar"
                        aria-label="更新下载进度"
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
          <div className="settings-toast" role="status">
            <span>{updateToast}</span>
            <button
              type="button"
              aria-label="关闭更新提示"
              onClick={() => setUpdateToast(undefined)}
            >
              <X size={14} />
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
