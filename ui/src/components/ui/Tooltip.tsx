import type { ReactNode } from "react";
import { cn } from "./cn";

export function Tooltip({
  content,
  children,
  className,
}: {
  content: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "group relative inline-flex cursor-help rounded-full outline-none focus-visible:outline-2 focus-visible:outline-text focus-visible:outline-offset-2",
        className,
      )}
      tabIndex={0}
      role="img"
      aria-label={content}
    >
      {children}
      <span
        role="tooltip"
        className="pointer-events-none absolute bottom-full start-1/2 z-20 mb-1.5 w-max max-w-64 -translate-x-1/2 rounded-sm bg-text px-2 py-1.5 font-sans text-sm font-normal leading-snug text-background opacity-0 shadow-control-subtle transition-opacity group-hover:opacity-100 group-focus:opacity-100"
      >
        {content}
      </span>
    </span>
  );
}
