import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown } from "lucide-react";

export interface SettingsSelectOption<T extends string> {
  value: T;
  label: string;
  description?: string;
  disabled?: boolean;
}

export default function SettingsSelect<T extends string>({
  value,
  options,
  ariaLabel,
  className,
  disabled = false,
  onChange,
}: {
  value: T;
  options: readonly SettingsSelectOption<T>[];
  ariaLabel: string;
  className?: string;
  disabled?: boolean;
  onChange: (value: T) => void;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent): void => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, [open]);

  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  const activeOption = options.find((option) => option.value === value);

  const closeAndRestoreFocus = (): void => {
    setOpen(false);
    triggerRef.current?.focus();
  };

  const focusOption = (direction: 1 | -1): void => {
    const items = Array.from(
      menuRef.current?.querySelectorAll<HTMLButtonElement>(
        '[role="option"]:not(:disabled)',
      ) ?? [],
    );
    if (items.length === 0) return;
    const currentIndex = items.indexOf(document.activeElement as HTMLButtonElement);
    const fallback = items.findIndex(
      (item) => item.getAttribute("aria-selected") === "true",
    );
    const startIndex = currentIndex >= 0 ? currentIndex : Math.max(fallback, 0);
    items[(startIndex + direction + items.length) % items.length]?.focus();
  };

  return (
    <div
      className={[
        "settings-select",
        className,
        open ? "open" : undefined,
        disabled ? "disabled" : undefined,
      ]
        .filter(Boolean)
        .join(" ")}
      ref={rootRef}
      onKeyDown={(event) => {
        if (event.key === "Escape" && open) {
          event.stopPropagation();
          closeAndRestoreFocus();
        } else if (open && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
          event.preventDefault();
          focusOption(event.key === "ArrowDown" ? 1 : -1);
        } else if (!open && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
          event.preventDefault();
          setOpen(true);
        }
      }}
    >
      <button
        ref={triggerRef}
        className="settings-select-trigger"
        type="button"
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
      >
        <span>{activeOption?.label ?? value}</span>
        <ChevronDown size={14} />
      </button>
      {open && (
        <div
          ref={menuRef}
          className="settings-select-menu"
          role="listbox"
          aria-label={ariaLabel}
        >
          {options.map((option, index) => {
            const selected = option.value === value;
            return (
              <button
                key={`${option.value}-${index}`}
                className={selected ? "selected" : undefined}
                type="button"
                role="option"
                aria-selected={selected}
                disabled={option.disabled}
                onClick={() => {
                  onChange(option.value);
                  closeAndRestoreFocus();
                }}
              >
                <span className="settings-select-option-copy">
                  <span>{option.label}</span>
                  {option.description && <small>{option.description}</small>}
                </span>
                {selected && <Check size={14} />}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
