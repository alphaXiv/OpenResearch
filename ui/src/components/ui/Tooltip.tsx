import { useEffect, useRef, type ReactNode } from "react";
import { cn } from "./cn";

export function Tooltip({
  content,
  children,
  className,
  interactive = false,
  disabled = false,
  side = "top",
}: {
  content: string;
  children: ReactNode;
  className?: string;
  interactive?: boolean;
  disabled?: boolean;
  /** `start` opens beside the trigger, toward the reading start, so rows in a list stay visible. */
  side?: "top" | "start";
}) {
  const triggerRef = useRef<HTMLSpanElement>(null);
  const tooltipRef = useRef<HTMLSpanElement>(null);

  function show() {
    if (disabled) return;
    const trigger = triggerRef.current;
    const tooltip = tooltipRef.current;
    if (!trigger || !tooltip) return;
    if (!tooltip.matches(":popover-open")) tooltip.showPopover();
    const anchor = trigger.getBoundingClientRect();
    const bounds = tooltip.getBoundingClientRect();
    if (side === "start") {
      const rtl = getComputedStyle(trigger).direction === "rtl";
      const beside = rtl ? anchor.right + 6 : anchor.left - bounds.width - 6;
      if (beside >= 8 && beside + bounds.width <= window.innerWidth - 8) {
        const middle = anchor.top + anchor.height / 2 - bounds.height / 2;
        tooltip.style.left = `${beside}px`;
        tooltip.style.top = `${Math.max(8, Math.min(middle, window.innerHeight - bounds.height - 8))}px`;
        return;
      }
    }
    const left = Math.max(8, Math.min(anchor.left + anchor.width / 2 - bounds.width / 2, window.innerWidth - bounds.width - 8));
    const above = anchor.top - bounds.height - 6;
    const top = above >= 8 ? above : anchor.bottom + 6;
    tooltip.style.left = `${left}px`;
    tooltip.style.top = `${Math.max(8, Math.min(top, window.innerHeight - bounds.height - 8))}px`;
  }

  function hide() {
    const trigger = triggerRef.current;
    if (!trigger?.matches(":hover, :focus-visible") && !trigger?.querySelector(":focus-visible")) tooltipRef.current?.hidePopover();
  }

  useEffect(() => {
    if (disabled) tooltipRef.current?.hidePopover();
  }, [disabled]);

  useEffect(() => {
    const dismiss = () => tooltipRef.current?.hidePopover();
    window.addEventListener("scroll", dismiss, true);
    window.addEventListener("resize", dismiss);
    return () => {
      dismiss();
      window.removeEventListener("scroll", dismiss, true);
      window.removeEventListener("resize", dismiss);
    };
  }, []);

  return (
    <span
      ref={triggerRef}
      className={cn(
        "group relative inline-flex rounded-full outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2",
        !interactive && "cursor-help",
        className,
      )}
      tabIndex={interactive ? undefined : 0}
      role={interactive ? undefined : "img"}
      aria-label={interactive ? undefined : content}
      onPointerDown={() => tooltipRef.current?.hidePopover()}
      onMouseEnter={show}
      onMouseLeave={hide}
      onFocus={(event) => {
        if (event.target.matches(":focus-visible")) show();
      }}
      onBlur={hide}
      onKeyDown={(event) => {
        if (event.key === "Escape" && tooltipRef.current?.matches(":popover-open")) {
          event.preventDefault();
          event.stopPropagation();
          tooltipRef.current.hidePopover();
        }
      }}
    >
      {children}
      <span
        ref={tooltipRef}
        popover="manual"
        role="tooltip"
        className="pointer-events-none fixed inset-auto m-0 w-max max-w-64 whitespace-normal rounded-sm border-0 bg-text px-2 py-1.5 font-sans text-sm font-normal leading-snug text-background shadow-control-subtle"
      >
        {content}
      </span>
    </span>
  );
}
