import { useMutation, useQuery } from "@tanstack/react-query";
import { setScopedQueryData } from "../queries/client";
import { getLitSourcesQuery } from "../queries/settings";
import { m } from "../paraglide/messages.js";
// Literature-source toggles shown inline in the composer chat-settings panel:
// which sources discovery and paper reading may use. State lives in settings.json
// (same `/api/settings/lit-sources` endpoint the CLI enforces).

import { setLitSources } from "../api";
import { LitSourceLogo, LIT_SOURCE_NAME, type LitSource } from "./LitSourceLogo";
import { MenuItem, SwitchIndicator } from "./ui";

const LIT_SOURCES: LitSource[] = ["alphaxiv", "openalex", "biorxiv"];

export function LitSourcesList() {
  const options = getLitSourcesQuery();
  const { data: settings } = useQuery(options);
  const mutation = useMutation({
    mutationFn: setLitSources,
    onSuccess: (settings) => setScopedQueryData(options.queryKey, settings),
  });
  const saving = mutation.isPending;
  const toggle = (key: LitSource) => {
    if (settings && !saving) mutation.mutate({ ...settings, [key]: !settings[key] });
  };

  if (!settings) return <div className="py-1.5 px-2 text-muted text-sm">{m.lit_sources_picker_loading()}</div>;

  return (
    <div className="flex flex-col">
      {LIT_SOURCES.map((key) => {
        const on = settings[key];
        return (
          <MenuItem
            key={key}
            type="button"
            role="switch"
            aria-checked={on}

            disabled={saving}
            onClick={() => toggle(key)}
          >
            <span className="inline-flex items-center gap-[9px]">
              <LitSourceLogo source={key} size={16} decorative />
              {LIT_SOURCE_NAME[key]}
            </span>
            <SwitchIndicator checked={on} aria-hidden="true" />
          </MenuItem>
        );
      })}
    </div>
  );
}
