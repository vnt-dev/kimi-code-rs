import type { ReactNode } from "react";

export function Collapsible({
  open,
  className = "",
  children,
}: {
  open: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      className={`collapsible ${open ? "open" : ""} ${className}`.trim()}
      aria-hidden={!open}
    >
      <div className="collapsible-inner">{children}</div>
    </div>
  );
}
