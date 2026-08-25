// Literature-source toggles shown inline in the composer chat-settings panel:
// which sources discovery and paper reading may use. State lives in settings.json
// (same `/api/settings/lit-sources` endpoint the CLI enforces).

import { useEffect, useState } from "react";

import { getLitSources, setLitSources, type LitSourcesSettings } from "../api";
import { LitSourceLogo, LIT_SOURCE_NAME, type LitSource } from "./LitSourceLogo";
import { MODEL_ITEM_CLASS_NAME, SETTINGS_SWITCH_CLASS_NAME } from "../styleClasses";

const LIT_SOURCES: LitSource[] = ["alphaxiv", "openalex", "biorxiv"];

// Remembered across mounts so reopening the panel shows the last values
// immediately instead of flashing "Loading…" while it revalidates.
let cachedSettings: LitSourcesSettings | null = null;

export function LitSourcesList() {
  const [settings, setSettings] = useState<LitSourcesSettings | null>(cachedSettings);
  const [saving, setSaving] = useState(false);

  const remember = (s: LitSourcesSettings) => {
    cachedSettings = s;
    setSettings(s);
  };

  useEffect(() => {
    void getLitSources()
      .then(remember)
      .catch(() => {});
  }, []);

  const toggle = (key: LitSource) => {
    if (!settings || saving) return;
    setSaving(true);
    void setLitSources({ ...settings, [key]: !settings[key] })
      .then(remember)
      .catch(() => {})
      .finally(() => setSaving(false));
  };

  if (!settings) return <div className="py-1.5 px-2 text-muted text-sm">Loading…</div>;

  return (
    <div className="flex flex-col">
      {LIT_SOURCES.map((key) => {
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
            <span className="inline-flex items-center gap-[9px]">
              <LitSourceLogo source={key} size={16} decorative />
              {LIT_SOURCE_NAME[key]}
            </span>
            <span className={`${SETTINGS_SWITCH_CLASS_NAME} ${on ? "on" : ""}`} aria-hidden="true">
              <span />
            </span>
          </button>
        );
      })}
    </div>
  );
}
