import { useEffect, useRef } from "react";
import { X } from "lucide-react";

import type { ColorScheme } from "./appearance";

export default function SettingsDialog({
  colorScheme,
  onColorSchemeChange,
  onClose,
}: {
  colorScheme: ColorScheme;
  onColorSchemeChange: (colorScheme: ColorScheme) => void;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);

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
              className="settings-tab active"
              type="button"
              aria-current="page"
            >
              通用
            </button>
          </nav>

          <main className="settings-dialog-content">
            <section className="settings-section" aria-labelledby="appearance-heading">
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
          </main>
        </div>
      </div>
    </div>
  );
}
