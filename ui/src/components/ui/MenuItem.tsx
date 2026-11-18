import type { ButtonHTMLAttributes, Ref } from "react";
import { cn } from "./cn";

/** A menu's surface and its section headings, shared so every menu spaces the same. */
export const MENU_POPUP = "overflow-y-auto rounded-lg border border-border bg-background p-1 shadow-dropdown outline-none";
export const MENU_LABEL = "px-2 pt-1.5 pb-1 text-sm text-muted";
export const MENU_SEPARATOR = "m-1 h-px bg-border-variant";

export function MenuItem({ active = false, danger = false, size = "default", className, ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { title?: never; active?: boolean; danger?: boolean; size?: "default" | "compact"; ref?: Ref<HTMLButtonElement> }) {
  return (
    <button
      className={cn(
        // Inset, so a scrolling menu never clips the focus ring.
        "model-item flex w-full items-center justify-between gap-2 rounded-sm px-2 text-start transition-[background,color] duration-120 ease-standard hover:bg-surface focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:-outline-offset-2 disabled:cursor-default disabled:opacity-45 [&_.model-id]:block [&_.model-id]:text-xs [&_.model-id]:text-muted",
        size === "compact" ? "min-h-6 py-0.5 text-menu" : "min-h-8 py-1.5 text-sm",
        active && "bg-surface",
        danger && "text-accent-red hover:text-accent-red",
        className,
      )}
      {...props}
      title=""
    />
  );
}
