import type { AnchorHTMLAttributes, ButtonHTMLAttributes } from "react";
import { cn } from "./cn";

type ButtonVariant = "default" | "primary" | "ghost" | "danger" | "warning";
type ButtonSize = "default" | "small" | "large";

const BASE = [
  "btn inline-flex shrink-0 items-center justify-center gap-1.5 whitespace-nowrap border font-medium",
  "transition-[background,border-color,color] duration-120 ease-standard",
  "focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-text focus-visible:outline-offset-2",
  "disabled:cursor-default disabled:opacity-45",
].join(" ");

const VARIANTS: Record<ButtonVariant, string> = {
  default: "border-border bg-background text-text [&:hover:not(:disabled)]:bg-surface [&:active:not(:disabled)]:bg-highlight",
  primary: "border-primary bg-primary text-background [&:hover:not(:disabled)]:border-primary-hover [&:hover:not(:disabled)]:bg-primary-hover [&:active:not(:disabled)]:border-primary-active [&:active:not(:disabled)]:bg-primary-active",
  ghost: "border-transparent bg-transparent text-text [&:hover:not(:disabled)]:bg-surface [&:active:not(:disabled)]:bg-highlight [&.active]:bg-surface [&.active]:text-muted",
  danger: "border-border bg-background text-accent-red [&:hover:not(:disabled)]:bg-danger-hover [&:active:not(:disabled)]:bg-danger-active",
  warning: "border-accent-amber bg-background text-accent-amber [&:hover:not(:disabled)]:bg-accent-amber-subtle [&:active:not(:disabled)]:bg-highlight",
};

const SIZES: Record<ButtonSize, string> = {
  default: "h-8 rounded-md px-3.5 text-sm",
  small: "h-7 rounded-sm px-2.5 text-sm",
  large: "h-14 rounded-lg px-7 text-xl",
};

function classes(variant: ButtonVariant, size: ButtonSize, active: boolean, className?: string) {
  return cn(BASE, VARIANTS[variant], SIZES[size], active && "active", className);
}

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  active?: boolean;
  variant?: ButtonVariant;
  size?: ButtonSize;
};

export function Button({ active = false, variant = "default", size = "default", className, ...props }: ButtonProps) {
  return <button className={classes(variant, size, active, className)} {...props} />;
}

export type ButtonLinkProps = AnchorHTMLAttributes<HTMLAnchorElement> & {
  active?: boolean;
  variant?: ButtonVariant;
  size?: ButtonSize;
};

export function ButtonLink({ active = false, variant = "default", size = "default", className, ...props }: ButtonLinkProps) {
  return <a className={classes(variant, size, active, className)} {...props} />;
}
