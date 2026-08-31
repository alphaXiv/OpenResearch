import type { HTMLAttributes } from "react";
import { cn } from "./ui/cn";

type PaperTitleVariant = "header" | "list";

const VARIANTS: Record<PaperTitleVariant, string> = {
  header: "min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-base font-semibold text-text",
  list: "text-sm font-medium text-text",
};

export function PaperTitle({ variant = "list", className, ...props }: HTMLAttributes<HTMLSpanElement> & { variant?: PaperTitleVariant }) {
  return <span className={cn("title", VARIANTS[variant], className)} {...props} />;
}
