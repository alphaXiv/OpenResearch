import type { HTMLAttributes } from "react";
import { cn } from "./cn";

export type BadgeVariant = "default" | "success" | "error" | "warning";

const VARIANTS: Record<BadgeVariant, string> = {
  default: "border-transparent bg-surface text-subtext",
  success: "border-accent-green bg-accent-green-subtle text-accent-green",
  error: "border-accent-red bg-accent-red-subtle text-accent-red",
  warning: "border-accent-amber bg-accent-amber-subtle text-accent-amber",
};

export function Badge({ variant = "default", size = "default", className, ...props }: HTMLAttributes<HTMLSpanElement> & { variant?: BadgeVariant; size?: "default" | "small" }) {
  return (
    <span
      className={cn("badge inline-flex items-center rounded-full border py-px font-sans", size === "small" ? "px-1.5 text-xs font-normal" : "px-2 text-sm font-medium", VARIANTS[variant], className)}
      {...props}
    />
  );
}
