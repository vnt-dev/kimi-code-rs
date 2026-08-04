import {
  type ImgHTMLAttributes,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";

import { t } from "../../i18n";

interface PreviewableImageProps
  extends Omit<
    ImgHTMLAttributes<HTMLImageElement>,
    "alt" | "onClick" | "onKeyDown" | "src" | "title"
  > {
  src: string;
  alt: string;
  path?: string;
}

function visibleImagePath(src: string, path?: string): string | undefined {
  const explicitPath = path?.trim();
  if (explicitPath) return explicitPath;
  if (/^(?:data|blob):/i.test(src)) return undefined;
  return src;
}

export function PreviewableImage({
  src,
  alt,
  path,
  className,
  ...imageProps
}: PreviewableImageProps) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLImageElement>(null);
  const displayedPath = visibleImagePath(src, path);

  const close = useCallback(() => {
    setOpen(false);
    window.requestAnimationFrame(() => triggerRef.current?.focus());
  }, []);

  useEffect(() => {
    if (!open) return;
    const closeOnEscape = (event: globalThis.KeyboardEvent): void => {
      if (event.key === "Escape") close();
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, [close, open]);

  const openPreview = (): void => setOpen(true);

  return (
    <>
      <span
        className="previewable-image"
        data-image-path={displayedPath}
      >
        <img
          {...imageProps}
          ref={triggerRef}
          className={className}
          src={src}
          alt={alt}
          title={displayedPath}
          role="button"
          tabIndex={0}
          aria-haspopup="dialog"
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            openPreview();
          }}
          onKeyDown={(event) => {
            if (event.key !== "Enter" && event.key !== " ") return;
            event.preventDefault();
            event.stopPropagation();
            openPreview();
          }}
        />
      </span>
      {open &&
        createPortal(
          <div className="image-preview-backdrop" onMouseDown={close}>
            <section
              className="image-preview-dialog"
              role="dialog"
              aria-modal="true"
              aria-label={alt}
              onMouseDown={(event) => event.stopPropagation()}
            >
              <button
                className="image-preview-close"
                type="button"
                aria-label={t("window.close")}
                onClick={close}
                autoFocus
              >
                <X size={20} />
              </button>
              <img src={src} alt={alt} title={displayedPath} />
              {displayedPath && (
                <div className="image-preview-path" title={displayedPath}>
                  {displayedPath}
                </div>
              )}
            </section>
          </div>,
          document.body,
        )}
    </>
  );
}
