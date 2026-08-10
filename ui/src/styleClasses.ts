export const ICON_BUTTON_BASE_CLASS_NAME = [
  "icon-btn [position:relative] [display:inline-flex] [align-items:center] [justify-content:center]",
  "[color:var(--subtext)] [&:hover]:[color:var(--text)] [&:hover]:[background:var(--surface)]",
  "[.chat-header.rail-hidden_>_&:first-child]:[margin-right:12px] [&.active]:[color:var(--primary)]",
  "[&.active]:[background:var(--surface)]",
].join(" ");

export const ICON_BUTTON_CLASS_NAME = [
  ICON_BUTTON_BASE_CLASS_NAME,
  "[width:28px] [height:28px]",
].join(" ");

export const MODEL_ITEM_CLASS_NAME = [
  "model-item [&.danger]:[color:var(--accent-red)] [&.danger:hover]:[color:var(--accent-red)] [display:flex]",
  "[align-items:center] [justify-content:space-between] [gap:8px] [width:100%] [text-align:left] [padding:6px_8px]",
  "[font-size:var(--fs-md)] [border-radius:var(--radius-sm)] [&:hover]:[background:var(--surface)]",
  "[&_.model-id]:[display:block] [&_.model-id]:[font-family:var(--mono)] [&_.model-id]:[font-size:var(--fs-2xs)]",
  "[&_.model-id]:[color:var(--muted)]",
].join(" ");

export const SETTINGS_LOADING_CLASS_NAME = [
  "settings-loading [display:flex] [align-items:center] [gap:8px] [color:var(--subtext)] [font-size:var(--fs-md)]",
  "[padding:4px_0]",
].join(" ");

export const SPINNER_CLASS_NAME = [
  "spinner [width:13px] [height:13px] [border:2px_solid_var(--border)] [border-top-color:var(--primary)]",
  "[border-radius:50%] [animation:spin_0.8s_linear_infinite] [flex-shrink:0]",
].join(" ");

export const MONO_CLASS_NAME = "mono [font-family:var(--mono)] [font-size:var(--fs-sm)]";

export const TAB_BODY_CLASS_NAME =
  "tab-body [flex:1] [min-height:0] [position:relative] [display:flex] [flex-direction:column]";

export const CODE_TAB_BODY_CLASS_NAME =
  "code-tab-body [flex:1] [min-height:0] [overflow:auto] [background:var(--base)]";

export const CODE_TAB_NOTE_CLASS_NAME = [
  "code-tab-note [padding:8px_16px] [font-size:var(--fs-sm)] [color:var(--muted)]",
  "[border-bottom:1px_solid_var(--border-variant)] [flex-shrink:0]",
].join(" ");

export const PAPER_TITLE_CLASS_NAME = [
  "title [.chat-header_&]:[font-size:var(--fs-base)] [.chat-header_&]:[font-weight:var(--fw-semibold)]",
  "[.chat-header_&]:[color:var(--text)] [.chat-header_&]:[flex:1] [.chat-header_&]:[min-width:0]",
  "[.chat-header_&]:[overflow:hidden] [.chat-header_&]:[text-overflow:ellipsis]",
  "[.chat-header_&]:[white-space:nowrap]",
].join(" ");

export const BUTTON_CLASS_NAME = [
  "btn [display:inline-flex] [align-items:center] [justify-content:center] [gap:6px] [padding:6px_14px]",
  "[font-size:var(--fs-sm)] [font-weight:var(--fw-semibold)] [border:1px_solid_var(--border)]",
  "[border-radius:var(--radius-md)] [background:var(--base)] [color:var(--text)] [white-space:nowrap]",
  "[transition:background_120ms_ease,_border-color_120ms_ease,_color_120ms_ease]",
  "[&:hover:not(:disabled)]:[background:var(--surface)] [&:active:not(:disabled)]:[background:var(--highlight)]",
  "[&:disabled]:[opacity:0.45] [&:disabled]:[cursor:default] [&.primary]:[background:var(--primary)]",
  "[&.primary]:[border-color:var(--primary)] [&.primary]:[color:var(--base)]",
  "[&.primary:hover:not(:disabled)]:[background:color-mix(in_oklab,_var(--primary)_88%,_var(--text))]",
  "[&.primary:hover:not(:disabled)]:[border-color:color-mix(in_oklab,_var(--primary)_88%,_var(--text))]",
  "[&.primary:active:not(:disabled)]:[background:color-mix(in_oklab,_var(--primary)_80%,_var(--text))]",
  "[&.primary:active:not(:disabled)]:[border-color:color-mix(in_oklab,_var(--primary)_80%,_var(--text))]",
  "[&.danger]:[color:var(--accent-red)]",
  "[&.danger:hover:not(:disabled)]:[background:color-mix(in_oklab,_var(--accent-red)_8%,_transparent)]",
  "[&.danger:active:not(:disabled)]:[background:color-mix(in_oklab,_var(--accent-red)_14%,_transparent)]",
  "[&.ghost]:[border-color:transparent] [&.ghost]:[color:var(--text)]",
  "[&.ghost:hover:not(:disabled)]:[color:var(--text)] [&.ghost:hover:not(:disabled)]:[background:var(--surface)]",
  "[&.sm]:[padding:3px_9px] [&.sm]:[font-size:var(--fs-xs)] [&.sm]:[border-radius:var(--radius-sm)]",
].join(" ");

export const SMALL_BUTTON_CLASS_NAME = `${BUTTON_CLASS_NAME} sm`;
export const PRIMARY_BUTTON_CLASS_NAME = `${BUTTON_CLASS_NAME} primary`;
export const GHOST_BUTTON_CLASS_NAME = `${BUTTON_CLASS_NAME} ghost`;
export const SMALL_PRIMARY_BUTTON_CLASS_NAME = `${SMALL_BUTTON_CLASS_NAME} primary`;

export const BADGE_CLASS_NAME = [
  "badge [display:inline-flex] [align-items:center] [font-family:var(--sans)] [font-size:var(--fs-xs)]",
  "[font-weight:var(--fw-medium)] [padding:1px_7px] [border:1px_solid_var(--border)]",
  "[border-radius:var(--radius-sm)] [color:var(--text)] [&.ok]:[color:var(--accent-green)]",
  "[&.ok]:[border-color:var(--accent-green)] [&.ok]:[background:var(--accent-green-subtle)]",
  "[&.err]:[color:var(--accent-red)] [&.err]:[border-color:var(--accent-red)]",
  "[&.err]:[background:var(--accent-red-subtle)] [&.warn]:[color:var(--accent-amber)]",
  "[&.warn]:[border-color:var(--accent-amber)] [&.warn]:[background:var(--accent-amber-subtle)]",
].join(" ");

export const ERROR_BADGE_CLASS_NAME = `${BADGE_CLASS_NAME} err`;
export const SUCCESS_BADGE_CLASS_NAME = `${BADGE_CLASS_NAME} ok`;
export const WARNING_BADGE_CLASS_NAME = `${BADGE_CLASS_NAME} warn`;

export const STATUS_BADGE_CLASS_NAME = [
  "status-badge [display:inline-flex] [align-items:center] [gap:6px] [font-size:var(--fs-sm)]",
  "[font-weight:var(--fw-medium)] [color:var(--text)] [white-space:nowrap] [&_.dot]:[width:7px]",
  "[&_.dot]:[height:7px] [&_.dot]:[border-radius:50%] [&_.dot]:[background:currentColor]",
  "[&_.dot]:[flex-shrink:0] [&.live_.dot]:[animation:or-pulse_1.2s_ease-in-out_infinite]",
  "[&.st-done_.dot]:[color:var(--accent-green)] [&.st-done_>_svg]:[color:var(--accent-green)]",
  "[&.st-failed_.dot]:[color:var(--accent-red)] [&.st-running_.dot]:[color:var(--accent-teal)]",
  "[&.st-starting_.dot]:[color:var(--accent-amber)] [&.st-cancelling_.dot]:[color:var(--accent-orange)]",
  "[&.st-cancelled_.dot]:[color:var(--accent-orange)] [&.st-editing_.dot]:[color:var(--accent-purple)]",
  "[&.st-idle_.dot]:[color:var(--muted)]",
].join(" ");
