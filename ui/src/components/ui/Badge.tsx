import type { HTMLAttributes } from "react";
import { cn } from "./cn";

export type BadgeVariant = "default" | "success" | "error" | "warning";

const VARIANTS: Record<BadgeVariant, string> = {
  default: "border-transparent bg-surface text-subtext",
  success: "border-accent-green bg-accent-green-subtle text-accent-green",
  error: "border-accent-red bg-accent-red-subtle text-accent-red",
  warning: "border-accent-amber bg-accent-amber-subtle text-accent-amber",
};

export function Badge({ variant = "default", className, ...props }: HTMLAttributes<HTMLSpanElement> & { variant?: BadgeVariant }) {
  return (
    <span
      className={cn("badge inline-flex items-center rounded-full border px-2 py-px font-sans text-sm font-medium", VARIANTS[variant], className)}
      {...props}
    />
  );
}
