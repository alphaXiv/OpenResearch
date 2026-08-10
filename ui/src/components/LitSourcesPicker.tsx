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
    <div className="option-picker [position:relative] [display:inline-flex]" ref={ref}>
      <button
        type="button"
        className="composer-bare [display:inline-flex] [align-items:center] [gap:3px] [font-size:var(--fs-md)] [color:var(--text)] [padding:5px_4px] [border-radius:var(--radius-sm)] [&:hover]:[color:var(--text)] [&.context-ring]:[display:inline-flex] [&.context-ring]:[align-items:center] [&.context-ring]:[margin-right:8px]"
        title="Literature sources for orx lit / orx paper"
        aria-label="Literature sources"
        onClick={() => setOpen((v) => !v)}
      >
        <ToggleRight size={16} />
      </button>
      {open && (
        <div className="option-menu [position:absolute] [bottom:calc(100%_+_8px)] [left:0] [max-height:380px] [display:flex] [flex-direction:column] [background:var(--base)] [border:1px_solid_var(--border)] [border-radius:var(--radius-lg)] [box-shadow:0_12px_32px_rgba(0,_0,_0,_0.18)] [z-index:50] [overflow:hidden] [padding:6px] [&.align-right]:[left:auto] [&.align-right]:[right:0] [&.drop-down]:[bottom:auto] [&.drop-down]:[top:calc(100%_+_4px)] [&.session-menu]:[left:auto] [&.session-menu]:[right:6px] [&.session-menu]:[top:calc(100%_-_2px)] [&.session-menu]:[min-width:140px] lit-sources-menu [min-width:210px]">
          <div className="model-group [display:flex] [align-items:center] [justify-content:space-between] [gap:8px] [font-size:var(--fs-xs)] [font-weight:var(--fw-medium)] [color:var(--muted)] [padding:8px_8px_4px]">Literature sources</div>
          {!settings ? (
            <div className="lit-sources-loading [padding:6px_8px] [color:var(--muted)] [font-size:var(--fs-sm)]">Loading…</div>
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
                  <span className="lit-source-item-label [display:inline-flex] [align-items:center] [gap:9px]">
                    <LitSourceLogo source={key} size={16} decorative />
                    {LIT_SOURCE_NAME[key]}
                  </span>
                  <span className={`settings-switch [position:relative] [flex:0_0_auto] [width:38px] [height:22px] [border:1px_solid_var(--border)] [border-radius:var(--radius-full)] [background:var(--surface)] [transition:background_120ms_ease,_border-color_120ms_ease] [&_span]:[position:absolute] [&_span]:[top:3px] [&_span]:[left:3px] [&_span]:[width:14px] [&_span]:[height:14px] [&_span]:[border-radius:50%] [&_span]:[background:var(--muted)] [&_span]:[transition:transform_120ms_ease,_background_120ms_ease] [&.on]:[border-color:var(--primary)] [&.on]:[background:var(--primary)] [&.on_span]:[background:var(--base)] [&.on_span]:[transform:translateX(16px)] [&:disabled]:[opacity:0.45] [&:disabled]:[cursor:default] [&:focus-visible]:[outline:2px_solid_var(--text)] [&:focus-visible]:[outline-offset:2px] ${on ? "on" : ""}`} aria-hidden="true">
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
