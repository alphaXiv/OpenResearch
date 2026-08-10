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
      className={`tab [&.closable]:[max-width:150px] [&.closable]:[padding-right:2px] [&_.tab-label]:[display:grid] [&_.tab-label]:[grid-template-columns:minmax(0,_1fr)] [&_.tab-label]:[min-width:0] [&_.tab-label]:[overflow:hidden] [&_.tab-label_>_span]:[grid-area:1_/_1] [&_.tab-label_>_span]:[overflow:hidden] [&_.tab-label_>_span]:[text-overflow:ellipsis] [&_.tab-label_>_span]:[white-space:nowrap] [&_.tab-label::after]:[grid-area:1_/_1] [&_.tab-label::after]:[overflow:hidden] [&_.tab-label::after]:[text-overflow:ellipsis] [&_.tab-label::after]:[white-space:nowrap] [&_.tab-label::after]:[content:attr(data-label)] [&_.tab-label::after]:[visibility:hidden] [&_.tab-label::after]:[font-weight:var(--fw-medium)] [&_.tab-close]:[display:inline-flex] [&_.tab-close]:[align-items:center] [&_.tab-close]:[justify-content:center] [&_.tab-close]:[width:14px] [&_.tab-close]:[height:14px] [&_.tab-close]:[border-radius:var(--radius-xs)] [&_.tab-close]:[color:var(--muted)] [&_.tab-close]:[flex-shrink:0] [&_.tab-close:hover]:[background:color-mix(in_oklab,_var(--text)_15%,_transparent)] [&_.tab-close:hover]:[color:var(--text)] [position:relative] [display:inline-flex] [align-items:center] [gap:5px] [height:32px] [padding:0_8px] [border:1px_solid_transparent] [border-bottom:none] [border-radius:var(--radius-md)_var(--radius-md)_0_0] [font-size:var(--fs-sm)] [font-weight:var(--fw-regular)] [color:var(--subtext)] [white-space:nowrap] [flex-shrink:0] [&:hover]:[background:var(--surface)] [&:hover]:[color:var(--text)] [&:not(.active)_+_.tab:not(.active)::before]:[content:''] [&:not(.active)_+_.tab:not(.active)::before]:[position:absolute] [&:not(.active)_+_.tab:not(.active)::before]:[top:10px] [&:not(.active)_+_.tab:not(.active)::before]:[bottom:10px] [&:not(.active)_+_.tab:not(.active)::before]:[left:-1px] [&:not(.active)_+_.tab:not(.active)::before]:[width:1px] [&:not(.active)_+_.tab:not(.active)::before]:[background:var(--border)] [&.active]:[border-color:var(--border)] [&.active]:[background:var(--base)] [&.active]:[color:var(--text)] [&.active]:[font-weight:var(--fw-medium)] [&.active::after]:[content:''] [&.active::after]:[position:absolute] [&.active::after]:[right:0] [&.active::after]:[bottom:-1px] [&.active::after]:[left:0] [&.active::after]:[height:1px] [&.active::after]:[background:var(--base)] closable ${active ? "active" : ""}`}
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
