import { useEffect, useRef, type ReactNode } from "react";
import { cn } from "./cn";

export function Tooltip({
  content,
  children,
  className,
}: {
  content: string;
  children: ReactNode;
  className?: string;
}) {
  const triggerRef = useRef<HTMLSpanElement>(null);
  const tooltipRef = useRef<HTMLSpanElement>(null);

  function show() {
    const trigger = triggerRef.current;
    const tooltip = tooltipRef.current;
    if (!trigger || !tooltip) return;
    if (!tooltip.matches(":popover-open")) tooltip.showPopover();
    const anchor = trigger.getBoundingClientRect();
    const bounds = tooltip.getBoundingClientRect();
    const left = Math.max(8, Math.min(anchor.left + anchor.width / 2 - bounds.width / 2, window.innerWidth - bounds.width - 8));
    tooltip.style.left = `${left}px`;
    tooltip.style.top = `${Math.max(8, anchor.top - bounds.height - 6)}px`;
  }

  function hide() {
    if (!triggerRef.current?.matches(":hover, :focus")) tooltipRef.current?.hidePopover();
  }

  useEffect(() => {
    const dismiss = () => tooltipRef.current?.hidePopover();
    window.addEventListener("scroll", dismiss, true);
    window.addEventListener("resize", dismiss);
    return () => {
      window.removeEventListener("scroll", dismiss, true);
      window.removeEventListener("resize", dismiss);
    };
  }, []);

  return (
    <span
      ref={triggerRef}
      className={cn(
        "group relative inline-flex cursor-help rounded-full outline-none focus-visible:outline-2 focus-visible:outline-text focus-visible:outline-offset-2",
        className,
      )}
      tabIndex={0}
      role="img"
      aria-label={content}
      onMouseEnter={show}
      onMouseLeave={hide}
      onFocus={show}
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
