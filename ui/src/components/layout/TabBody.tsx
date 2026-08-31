import type { HTMLAttributes } from "react";
import { cn } from "../ui/cn";

export function TabBody({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("relative flex min-h-0 flex-1 flex-col", className)} {...props} />;
}

export function CodeTabBody({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("min-h-0 flex-1 overflow-auto bg-background", className)} {...props} />;
}

export function CodeTabNote({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("shrink-0 border-b border-b-border-variant px-4 py-2 text-sm text-muted", className)} {...props} />;
}
