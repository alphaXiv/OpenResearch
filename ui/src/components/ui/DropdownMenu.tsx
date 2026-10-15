import * as DropdownPrimitive from "@radix-ui/react-dropdown-menu";
import type { ComponentProps, RefObject } from "react";
import { getTextDirection } from "../../paraglide/runtime.js";
import { MenuItem } from "./MenuItem";
import { cn } from "./cn";

export function DropdownMenu(props: ComponentProps<typeof DropdownPrimitive.Root>) {
  return <DropdownPrimitive.Root dir={getTextDirection()} modal={false} {...props} />;
}

export const DropdownMenuTrigger = DropdownPrimitive.Trigger;

export function DropdownMenuContent({
  ownerRef,
  restoreFocus = true,
  className,
  onInteractOutside,
  onCloseAutoFocus,
  onEscapeKeyDown,
  onClick,
  ...props
}: ComponentProps<typeof DropdownPrimitive.Content> & {
  ownerRef?: RefObject<HTMLElement | null>;
  restoreFocus?: boolean;
}) {
  return (
    <DropdownPrimitive.Portal>
      <DropdownPrimitive.Content
        align="end"
        sideOffset={4}
        collisionPadding={8}
        hideWhenDetached
        loop
        className={cn("option-menu z-70 flex flex-col max-h-(--radix-dropdown-menu-content-available-height) overflow-y-auto min-w-35 rounded-lg border border-border bg-background p-1.5 shadow-menu", className)}
        {...props}
        onInteractOutside={(event) => {
          onInteractOutside?.(event);
          const target = event.detail.originalEvent.target;
          if (target instanceof Node && ownerRef?.current?.contains(target)) event.preventDefault();
        }}
        onCloseAutoFocus={(event) => {
          onCloseAutoFocus?.(event);
          if (!restoreFocus) event.preventDefault();
        }}
        onEscapeKeyDown={(event) => {
          onEscapeKeyDown?.(event);
          event.stopPropagation();
        }}
        onClick={(event) => {
          onClick?.(event);
          event.stopPropagation();
        }}
      />
    </DropdownPrimitive.Portal>
  );
}

export function DropdownMenuItem({ danger, className, children, ...props }: ComponentProps<typeof DropdownPrimitive.Item> & { danger?: boolean }) {
  return (
    <DropdownPrimitive.Item asChild {...props}>
      <MenuItem danger={danger} className={cn("data-highlighted:bg-surface data-disabled:opacity-45", className)}>
        {children}
      </MenuItem>
    </DropdownPrimitive.Item>
  );
}
