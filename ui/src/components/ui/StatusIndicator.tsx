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

export function StatusIndicator({ tone = "neutral", live = false, className, children, ...props }: HTMLAttributes<HTMLSpanElement> & { tone?: StatusTone; live?: boolean }) {
  return (
    <span
      className={cn("status-badge inline-flex items-center gap-1.5 whitespace-nowrap text-sm font-medium text-text", className)}
      {...props}
    >
      <span className={cn("h-[7px] w-[7px] shrink-0 rounded-full bg-current", TONES[tone], live && "animate-[or-pulse_1.2s_ease-in-out_infinite]")} />
      {children}
    </span>
  );
}
