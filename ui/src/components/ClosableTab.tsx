import { X } from "lucide-react";

/** A closable tab in the right panel's tab strip (open experiments / files).
 *  The close "x" is a span, not a button — it can't nest inside the tab button. */
export function ClosableTab({
  active,
  label,
  icon,
  onSelect,
  onClose,
}: {
  active: boolean;
  label: string;
  /** Optional leading adornment, e.g. a busy dot or branch icon. */
  icon?: React.ReactNode;
  onSelect: () => void;
  onClose: () => void;
}) {
  return (
    <button
      className={`tab [&.closable]:max-w-37.5 [&.closable]:pr-0.5 [&_.tab-label]:grid [&_.tab-label]:grid-cols-[minmax(0,_1fr)] [&_.tab-label]:min-w-0 [&_.tab-label]:overflow-hidden [&_.tab-label_>_span]:[grid-area:1_/_1] [&_.tab-label_>_span]:overflow-hidden [&_.tab-label_>_span]:text-ellipsis [&_.tab-label_>_span]:whitespace-nowrap [&_.tab-label::after]:[grid-area:1_/_1] [&_.tab-label::after]:overflow-hidden [&_.tab-label::after]:text-ellipsis [&_.tab-label::after]:whitespace-nowrap [&_.tab-label::after]:content-[attr(data-label)] [&_.tab-label::after]:invisible [&_.tab-label::after]:font-medium [&_.tab-close]:inline-flex [&_.tab-close]:items-center [&_.tab-close]:justify-center [&_.tab-close]:w-3.5 [&_.tab-close]:h-3.5 [&_.tab-close]:rounded-xs [&_.tab-close]:text-muted [&_.tab-close]:shrink-0 [&_.tab-close:hover]:bg-[color-mix(in_oklab,_var(--text)_15%,_transparent)] [&_.tab-close:hover]:text-text relative inline-flex items-center gap-[5px] h-8 py-0 px-2 border border-transparent border-b-0 rounded-[var(--radius-md)_var(--radius-md)_0_0] text-sm font-normal text-subtext whitespace-nowrap shrink-0 [&:hover]:bg-surface [&:hover]:text-text [&:not(.active)_+_.tab:not(.active)::before]:content-[''] [&:not(.active)_+_.tab:not(.active)::before]:absolute [&:not(.active)_+_.tab:not(.active)::before]:top-2.5 [&:not(.active)_+_.tab:not(.active)::before]:bottom-2.5 [&:not(.active)_+_.tab:not(.active)::before]:-left-px [&:not(.active)_+_.tab:not(.active)::before]:w-px [&:not(.active)_+_.tab:not(.active)::before]:bg-border [&.active]:border-border [&.active]:bg-background [&.active]:text-text [&.active]:font-medium [&.active::after]:content-[''] [&.active::after]:absolute [&.active::after]:right-0 [&.active::after]:-bottom-px [&.active::after]:left-0 [&.active::after]:h-px [&.active::after]:bg-background closable ${active ? "active" : ""}`}
      onClick={onSelect}
      title={label}
    >
      {icon}
      <span className="tab-label" data-label={label}>
        <span>{label}</span>
      </span>
      <span
        role="button"
        className="tab-close"
        title="Close tab"
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
      >
        <X size={12} />
      </span>
    </button>
  );
}
