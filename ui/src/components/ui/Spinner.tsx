import type { HTMLAttributes } from "react";
import { cn } from "./cn";

export function Spinner({ className, ...props }: HTMLAttributes<HTMLSpanElement>) {
  return (
    <span
      className={cn("spinner h-[13px] w-[13px] shrink-0 animate-[spin_0.8s_linear_infinite] rounded-full border-2 border-border border-t-primary", className)}
      {...props}
    />
  );
}

export function LoadingRow({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn("flex items-center gap-2 px-0 py-1 text-sm text-subtext", className)}
      {...props}
    />
  );
}
