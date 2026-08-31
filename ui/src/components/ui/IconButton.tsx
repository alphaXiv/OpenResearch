import { forwardRef, type AnchorHTMLAttributes, type ButtonHTMLAttributes } from "react";
import { cn } from "./cn";

const BASE = [
  "icon-btn relative inline-flex shrink-0 items-center justify-center",
  "transition-[background,color] duration-120 ease-standard",
  "focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-text focus-visible:outline-offset-2",
  "disabled:cursor-default disabled:opacity-45",
  "[.chat-header.rail-hidden_>_&:first-child]:me-3",
].join(" ");

type IconButtonSize = "default" | "small";
type IconButtonVariant = "default" | "primary" | "stop";

const VARIANTS: Record<IconButtonVariant, string> = {
  default: "text-subtext [&:hover:not(:disabled)]:bg-surface [&:hover:not(:disabled)]:text-text [&:active:not(:disabled)]:bg-highlight [&.active]:bg-surface [&.active]:text-primary",
  primary: "bg-primary text-background [&:hover:not(:disabled)]:bg-primary-hover [&:active:not(:disabled)]:bg-primary-active",
  stop: "bg-surface text-text [&:hover:not(:disabled)]:bg-stop-hover [&:active:not(:disabled)]:bg-highlight",
};

const SIZES: Record<IconButtonSize, string> = {
  default: "h-8 w-8 rounded-md",
  small: "h-7 w-7 rounded-sm",
};

function classes(variant: IconButtonVariant, size: IconButtonSize, active: boolean, className?: string) {
  return cn(BASE, VARIANTS[variant], SIZES[size], active && "active", className);
}

export type IconButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  active?: boolean;
  size?: IconButtonSize;
  variant?: IconButtonVariant;
};

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { active = false, size = "default", variant = "default", className, ...props },
  ref,
) {
  return <button ref={ref} className={classes(variant, size, active, className)} {...props} />;
});

export type IconButtonLinkProps = AnchorHTMLAttributes<HTMLAnchorElement> & {
  active?: boolean;
  size?: IconButtonSize;
  variant?: IconButtonVariant;
};

export function IconButtonLink({ active = false, size = "default", variant = "default", className, ...props }: IconButtonLinkProps) {
  return <a className={classes(variant, size, active, className)} {...props} />;
}
