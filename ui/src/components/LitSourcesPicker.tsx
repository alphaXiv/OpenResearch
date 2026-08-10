// Composer-footer control: a switch icon that opens a small popover to toggle
// which literature sources `orx lit` / `orx paper` may use. The state lives in
// settings.json (same `/api/settings/lit-sources` endpoint the CLI enforces).

import { useEffect, useState } from "react";

import { ToggleRight } from "lucide-react";

import { getLitSources, setLitSources, type LitSourcesSettings } from "../api";
import { LitSourceLogo, LIT_SOURCE_NAME, type LitSource } from "./LitSourceLogo";
import { usePopover } from "./ModelPicker";
import { MODEL_ITEM_CLASS_NAME } from "../styleClasses";

const LIT_SOURCES: LitSource[] = ["alphaxiv", "openalex", "biorxiv"];

export function LitSourcesPicker() {
  const { open, setOpen, ref } = usePopover();
  const [settings, setSettings] = useState<LitSourcesSettings | null>(null);
  const [saving, setSaving] = useState(false);

  // Load lazily the first time the menu opens.
  useEffect(() => {
    if (!open || settings) return;
    void getLitSources()
      .then(setSettings)
      .catch(() => {});
  }, [open, settings]);

  const toggle = (key: LitSource) => {
    if (!settings || saving) return;
    setSaving(true);
    void setLitSources({ ...settings, [key]: !settings[key] })
      .then(setSettings)
      .catch(() => {})
      .finally(() => setSaving(false));
  };

  return (
    <div className="option-picker relative inline-flex" ref={ref}>
      <button
        type="button"
        className="composer-bare inline-flex items-center gap-[3px] text-md text-text py-[5px] px-1 rounded-sm [&:hover]:text-text [&.context-ring]:inline-flex [&.context-ring]:items-center [&.context-ring]:mr-2"
        title="Literature sources for orx lit / orx paper"
        aria-label="Literature sources"
        onClick={() => setOpen((v) => !v)}
      >
        <ToggleRight size={16} />
      </button>
      {open && (
        <div className="option-menu absolute bottom-[calc(100%_+_8px)] left-0 max-h-95 flex flex-col bg-background border border-border rounded-lg shadow-[0_12px_32px_rgba(0,_0,_0,_0.18)] z-50 overflow-hidden p-1.5 [&.align-right]:left-auto [&.align-right]:right-0 [&.drop-down]:bottom-auto [&.drop-down]:top-[calc(100%_+_4px)] [&.session-menu]:left-auto [&.session-menu]:right-1.5 [&.session-menu]:top-[calc(100%_-_2px)] [&.session-menu]:min-w-35 lit-sources-menu min-w-52.5">
          <div className="model-group flex items-center justify-between gap-2 text-xs font-medium text-muted pt-2 px-2 pb-1">Literature sources</div>
          {!settings ? (
            <div className="lit-sources-loading py-1.5 px-2 text-muted text-sm">Loading…</div>
          ) : (
            LIT_SOURCES.map((key) => {
              const on = settings[key];
              return (
                <button
                  key={key}
                  type="button"
                  role="switch"
                  aria-checked={on}
                  className={MODEL_ITEM_CLASS_NAME}
                  disabled={saving}
                  onClick={() => toggle(key)}
                >
                  <span className="lit-source-item-label inline-flex items-center gap-[9px]">
                    <LitSourceLogo source={key} size={16} decorative />
                    {LIT_SOURCE_NAME[key]}
                  </span>
                  <span className={`settings-switch relative flex-none w-9.5 h-5.5 border border-border rounded-full bg-surface transition-[background,border-color] duration-120 ease-standard [&_span]:absolute [&_span]:top-[3px] [&_span]:left-[3px] [&_span]:w-3.5 [&_span]:h-3.5 [&_span]:rounded-full [&_span]:bg-muted [&_span]:transition-[translate,background] [&_span]:duration-120 [&_span]:ease-standard [&.on]:border-primary [&.on]:bg-primary [&.on_span]:bg-background [&.on_span]:translate-x-4 [&:disabled]:opacity-45 [&:disabled]:cursor-default [&:focus-visible]:outline-2 [&:focus-visible]:outline-solid [&:focus-visible]:outline-text [&:focus-visible]:outline-offset-2 ${on ? "on" : ""}`} aria-hidden="true">
                    <span />
                  </span>
                </button>
              );
            })
          )}
        </div>
      )}
    </div>
  );
}
