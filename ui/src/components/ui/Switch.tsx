import type { ButtonHTMLAttributes, HTMLAttributes } from "react";
import { cn } from "./cn";

const BASE = [
  "relative h-5.5 w-9.5 flex-none rounded-full border border-border bg-surface",
  "transition-[background,border-color] duration-120 ease-standard",
  "[&_span]:absolute [&_span]:start-[3px] [&_span]:top-[3px] [&_span]:h-3.5 [&_span]:w-3.5",
  "[&_span]:rounded-full [&_span]:bg-muted [&_span]:transition-[translate,background] [&_span]:duration-120 [&_span]:ease-standard",
  "hover:border-border-strong",
  "disabled:cursor-default disabled:opacity-45 focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-text focus-visible:outline-offset-2",
].join(" ");

function classes(checked: boolean, className?: string) {
  return cn(BASE, checked && "border-primary bg-primary [&_span]:translate-x-4 [&_span]:bg-background", className);
}

export function Switch({ checked = false, className, children, ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { checked?: boolean }) {
  return <button role="switch" aria-checked={checked} className={classes(checked, className)} {...props}>{children ?? <span />}</button>;
}

export function SwitchIndicator({ checked = false, className, ...props }: HTMLAttributes<HTMLSpanElement> & { checked?: boolean }) {
  return <span className={classes(checked, className)} {...props}><span /></span>;
}
