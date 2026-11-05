import type { ComponentProps } from "react";
import { cn } from "./cn";

type InputVariant = "default" | "inline";

const VARIANTS: Record<InputVariant, string> = {
  default: "h-8 rounded-md border border-border bg-background px-2.5 py-1.5 focus:border-focus-border focus:ring-3 focus:ring-focus-ring",
  inline: "h-8 rounded-none border-x-0 border-t-0 border-b border-transparent bg-transparent px-0 py-0 focus:border-focus-border",
};

export type InputProps = ComponentProps<"input"> & {
  variant?: InputVariant;
};

export function Input({ variant = "default", className, ...props }: InputProps) {
  return (
    <input
      className={cn(
        "w-full font-sans text-sm font-normal text-text outline-none placeholder:text-muted disabled:cursor-default disabled:opacity-45",
        VARIANTS[variant],
        className,
      )}
      {...props}
    />
  );
}

export function Textarea({ variant = "default", className, ...props }: ComponentProps<"textarea"> & { variant?: InputVariant }) {
  return (
    <textarea
      className={cn(
        "w-full font-sans font-normal text-text outline-none placeholder:text-muted disabled:cursor-default disabled:opacity-45",
        variant === "inline"
          ? "min-h-8 max-h-32 field-sizing-content resize-none border-0 bg-transparent py-1 text-base focus-visible:ring-1 focus-visible:ring-focus-border rounded-sm"
          : "min-h-16 resize-y rounded-md border border-border bg-background px-2.5 py-1.5 text-sm focus:border-focus-border focus:ring-3 focus:ring-focus-ring",
        className,
      )}
      {...props}
    />
  );
}
