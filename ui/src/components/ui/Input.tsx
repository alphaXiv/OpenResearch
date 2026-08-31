import type { InputHTMLAttributes } from "react";
import { cn } from "./cn";

type InputVariant = "default" | "inline";

const VARIANTS: Record<InputVariant, string> = {
  default: "h-8 rounded-md border border-border bg-background px-2.5 py-1.5 focus:border-text",
  inline: "h-8 rounded-none border-x-0 border-t-0 border-b border-transparent bg-transparent px-0 py-0 focus:border-text",
};

export type InputProps = InputHTMLAttributes<HTMLInputElement> & {
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
