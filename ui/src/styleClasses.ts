export const ELEVATED_SURFACE_SHADOW_CLASS_NAME =
  "shadow-[0_6px_24px_color-mix(in_oklab,_var(--text)_5%,_transparent),_0_1px_4px_color-mix(in_oklab,_var(--text)_4%,_transparent)]";

export const ICON_BUTTON_BASE_CLASS_NAME = [
  "icon-btn relative inline-flex items-center justify-center",
  "text-subtext [&:hover]:text-text [&:hover]:bg-surface",
  "[.chat-header.rail-hidden_>_&:first-child]:me-3 [&.active]:text-primary",
  "[&.active]:bg-surface",
].join(" ");

export const ICON_BUTTON_CLASS_NAME = [
  ICON_BUTTON_BASE_CLASS_NAME,
  "w-7 h-7",
].join(" ");

export const COMPOSER_CONTROL_CLASS_NAME = [
  "inline-flex h-8 items-center rounded-md",
  "transition-[background,color] duration-150 ease-standard hover:bg-surface",
].join(" ");

export const COMPOSER_ICON_CONTROL_CLASS_NAME = [
  COMPOSER_CONTROL_CLASS_NAME,
  "w-8 shrink-0 justify-center text-text",
].join(" ");

export const MODEL_ITEM_CLASS_NAME = [
  "model-item [&.danger]:text-accent-red [&.danger:hover]:text-accent-red flex",
  "items-center justify-between gap-2 w-full text-start py-1.5 px-2",
  "text-md rounded-sm [&:hover]:bg-surface",
  "[&_.model-id]:block [&_.model-id]:font-mono [&_.model-id]:text-2xs",
  "[&_.model-id]:text-muted",
].join(" ");

export const SETTINGS_LOADING_CLASS_NAME = [
  "settings-loading flex items-center gap-2 text-subtext text-md",
  "py-1 px-0",
].join(" ");

export const SPINNER_CLASS_NAME = [
  "spinner w-[13px] h-[13px] border-2 border-border border-t-primary",
  "rounded-full animate-[spin_0.8s_linear_infinite] shrink-0",
].join(" ");

export const MONO_CLASS_NAME = "mono font-mono text-sm";

export const TAB_BODY_CLASS_NAME =
  "tab-body flex-1 min-h-0 relative flex flex-col";

export const CODE_TAB_BODY_CLASS_NAME =
  "code-tab-body flex-1 min-h-0 overflow-auto bg-background";

export const CODE_TAB_NOTE_CLASS_NAME = [
  "code-tab-note py-2 px-4 text-sm text-muted",
  "border-b border-b-border-variant shrink-0",
].join(" ");

export const PAPER_TITLE_CLASS_NAME = [
  "title [.chat-header_&]:text-base [.chat-header_&]:font-semibold",
  "[.chat-header_&]:text-text [.chat-header_&]:flex-1 [.chat-header_&]:min-w-0",
  "[.chat-header_&]:overflow-hidden [.chat-header_&]:text-ellipsis",
  "[.chat-header_&]:whitespace-nowrap",
].join(" ");

export const BUTTON_CLASS_NAME = [
  "btn inline-flex items-center justify-center gap-1.5 py-1.5 px-3.5",
  "text-sm font-semibold border border-border",
  "rounded-md bg-background text-text whitespace-nowrap",
  "transition-[background,border-color,color] duration-120 ease-standard",
  "[&:hover:not(:disabled)]:bg-surface [&:active:not(:disabled)]:bg-highlight",
  "[&:disabled]:opacity-45 [&:disabled]:cursor-default [&.primary]:bg-primary",
  "[&.primary]:border-primary [&.primary]:text-background",
  "[&.primary:hover:not(:disabled)]:bg-[color-mix(in_oklab,_var(--primary)_88%,_var(--text))]",
  "[&.primary:hover:not(:disabled)]:border-[color-mix(in_oklab,_var(--primary)_88%,_var(--text))]",
  "[&.primary:active:not(:disabled)]:bg-[color-mix(in_oklab,_var(--primary)_80%,_var(--text))]",
  "[&.primary:active:not(:disabled)]:border-[color-mix(in_oklab,_var(--primary)_80%,_var(--text))]",
  "[&.danger]:text-accent-red",
  "[&.danger:hover:not(:disabled)]:bg-[color-mix(in_oklab,_var(--accent-red)_8%,_transparent)]",
  "[&.danger:active:not(:disabled)]:bg-[color-mix(in_oklab,_var(--accent-red)_14%,_transparent)]",
  "[&.ghost]:border-transparent [&.ghost]:text-text",
  "[&.ghost:hover:not(:disabled)]:text-text [&.ghost:hover:not(:disabled)]:bg-surface",
  "[&.sm]:py-[3px] [&.sm]:px-[9px] [&.sm]:text-xs [&.sm]:rounded-sm",
].join(" ");

export const SMALL_BUTTON_CLASS_NAME = `${BUTTON_CLASS_NAME} sm`;
export const PRIMARY_BUTTON_CLASS_NAME = `${BUTTON_CLASS_NAME} primary`;
export const GHOST_BUTTON_CLASS_NAME = `${BUTTON_CLASS_NAME} ghost`;

export const BADGE_CLASS_NAME = [
  "badge inline-flex items-center font-sans text-xs",
  "font-medium py-px px-[7px] border border-border",
  "rounded-sm text-text [&.ok]:text-accent-green",
  "[&.ok]:border-accent-green [&.ok]:bg-accent-green-subtle",
  "[&.err]:text-accent-red [&.err]:border-accent-red",
  "[&.err]:bg-accent-red-subtle [&.warn]:text-accent-amber",
  "[&.warn]:border-accent-amber [&.warn]:bg-accent-amber-subtle",
].join(" ");

export const ERROR_BADGE_CLASS_NAME = `${BADGE_CLASS_NAME} err`;
export const SUCCESS_BADGE_CLASS_NAME = `${BADGE_CLASS_NAME} ok`;
export const WARNING_BADGE_CLASS_NAME = `${BADGE_CLASS_NAME} warn`;

export const STATUS_BADGE_CLASS_NAME = [
  "status-badge inline-flex items-center gap-1.5 text-sm",
  "font-medium text-text whitespace-nowrap [&_.dot]:w-[7px]",
  "[&_.dot]:h-[7px] [&_.dot]:rounded-full [&_.dot]:bg-current",
  "[&_.dot]:shrink-0 [&.live_.dot]:animate-[or-pulse_1.2s_ease-in-out_infinite]",
  "[&.st-done_.dot]:text-accent-green [&.st-done_>_svg]:text-accent-green",
  "[&.st-failed_.dot]:text-accent-red [&.st-running_.dot]:text-accent-teal",
  "[&.st-starting_.dot]:text-accent-amber [&.st-cancelling_.dot]:text-accent-orange",
  "[&.st-cancelled_.dot]:text-accent-orange [&.st-editing_.dot]:text-accent-purple",
  "[&.st-idle_.dot]:text-muted",
].join(" ");

export const SETTINGS_SWITCH_CLASS_NAME = [
  "settings-switch relative flex-none w-9.5 h-5.5 border border-border rounded-full",
  "bg-surface transition-[background,border-color] duration-120 ease-standard",
  "[&_span]:absolute [&_span]:top-[3px] [&_span]:start-[3px] [&_span]:w-3.5 [&_span]:h-3.5",
  "[&_span]:rounded-full [&_span]:bg-muted [&_span]:transition-[translate,background]",
  "[&_span]:duration-120 [&_span]:ease-standard [&.on]:border-primary [&.on]:bg-primary",
  "[&.on_span]:bg-background [&.on_span]:translate-x-4 rtl:[&.on_span]:-translate-x-4 [&:disabled]:opacity-45",
  "[&:disabled]:cursor-default [&:focus-visible]:outline-2 [&:focus-visible]:outline-solid",
  "[&:focus-visible]:outline-text [&:focus-visible]:outline-offset-2",
].join(" ");
