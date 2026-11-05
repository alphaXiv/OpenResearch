import type { HTMLAttributes } from "react";
import { cn } from "./cn";

export type StatusTone = "success" | "danger" | "info" | "warning" | "caution" | "accent" | "neutral";

const TONES: Record<StatusTone, string> = {
  success: "text-accent-green",
  danger: "text-accent-red",
  info: "text-accent-teal",
  warning: "text-accent-amber",
  caution: "text-accent-orange",
  accent: "text-accent-purple",
  neutral: "text-muted",
};

/** `stale` draws a hollow dot: the status is what was last seen, not live knowledge. */
export function StatusIndicator({ tone = "neutral", live = false, stale = false, className, children, ...props }: HTMLAttributes<HTMLSpanElement> & { tone?: StatusTone; live?: boolean; stale?: boolean }) {
  return (
    <span
      className={cn("status-badge inline-flex items-center gap-1.5 whitespace-nowrap text-sm font-medium text-text", className)}
      {...props}
    >
      <span className={cn("h-[7px] w-[7px] shrink-0 rounded-full", TONES[tone], stale ? "border-[1.5px] border-current" : "bg-current", live && !stale && "animate-[or-pulse_1.2s_ease-in-out_infinite]")} />
      {children}
    </span>
  );
}
