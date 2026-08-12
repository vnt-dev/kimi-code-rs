import {
  type ReactNode,
  isValidElement,
  memo,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { Check, Copy } from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import { t } from "../../i18n";
import { resolveMarkdownExternalUrl } from "../../markdownLinks";
import { openExternalUrl } from "../../transport";
import { PreviewableImage } from "./PreviewableImage";

function MarkdownCodeBlock({ children }: { children: ReactNode }) {
  const className = isValidElement<{ className?: string }>(children)
    ? children.props.className
    : undefined;
  const language = className?.match(/language-([^\s]+)/)?.[1] ?? "code";
  const code = isValidElement<{ children?: ReactNode }>(children)
    ? children.props.children
    : children;
  const text = typeof code === "string" ? code.replace(/\n$/, "") : "";
  const [copied, setCopied] = useState(false);

  const copyCode = async (): Promise<void> => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1400);
    } catch (error) {
      console.error("failed to copy Markdown code block", error);
    }
  };

  return (
    <div className="code-wrap">
      <div className="code-label">
        <span>{language}</span>
        <button
          type="button"
          className="code-copy"
          title={copied ? t("common.copied") : t("common.copy")}
          aria-label={copied ? t("common.copied") : t("common.copy")}
          onClick={() => void copyCode()}
        >
          {copied ? <Check size={13} /> : <Copy size={13} />}
        </button>
      </div>
      <pre>{children}</pre>
    </div>
  );
}

export const MarkdownMessage = memo(function MarkdownMessage({
  content,
}: {
  content: string;
}) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm]}
      components={{
        pre({ children }) {
          return <MarkdownCodeBlock>{children}</MarkdownCodeBlock>;
        },
        code({ className, children, ...props }) {
          return (
            <code className={className} {...props}>
              {children}
            </code>
          );
        },
        table({ children }) {
          return (
            <div className="markdown-table-wrap">
              <table>{children}</table>
            </div>
          );
        },
        img({ src, alt, ...props }) {
          if (!src) return <img {...props} alt={alt ?? ""} />;
          return (
            <PreviewableImage
              {...props}
              src={src}
              alt={alt ?? ""}
              path={src}
            />
          );
        },
        a({ children, href, ...props }) {
          const externalUrl = resolveMarkdownExternalUrl(href);
          return (
            <a
              {...props}
              href={externalUrl}
              target="_blank"
              rel="noopener noreferrer"
              onClick={(event) => {
                if (!externalUrl) {
                  event.preventDefault();
                  return;
                }
                event.preventDefault();
                void openExternalUrl(externalUrl).catch((error) => {
                  console.error("failed to open Markdown link", error);
                });
              }}
            >
              {children}
            </a>
          );
        },
      }}
    >
      {content}
    </ReactMarkdown>
  );
});

export function StreamingMarkdownMessage({
  content,
  active,
}: {
  content: string;
  active: boolean;
}) {
  const latestContent = useRef(content);
  latestContent.current = content;
  const [displayedContent, setDisplayedContent] = useState(content);

  useLayoutEffect(() => {
    if (!active) setDisplayedContent(content);
  }, [active, content]);

  useEffect(() => {
    if (!active) return;
    const interval = window.setInterval(() => {
      setDisplayedContent(latestContent.current);
    }, 80);
    return () => window.clearInterval(interval);
  }, [active]);

  return <MarkdownMessage content={displayedContent} />;
}
