import { useWorkspace } from "../workspace";
import { useQuery } from "@tanstack/react-query";

import { m } from "../paraglide/messages.js";
import { mutSetLitSources } from "../queries/settings";
// Literature-source toggles shown inline in the composer chat-settings panel:
// which sources discovery and paper reading may use. State lives in settings.json
// (same `/api/settings/lit-sources` endpoint the CLI enforces).

import { LitSourceLogo, LIT_SOURCE_NAME, type LitSource } from "./LitSourceLogo";
import { MenuItem, SwitchIndicator } from "./ui";

const LIT_SOURCES: LitSource[] = ["alphaxiv", "openalex", "biorxiv", "pubmed"];

export function LitSourcesList() {
  const workspace = useWorkspace();
  const { data: settings } = useQuery(workspace.queries.getLitSourcesQuery());

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
            onClick={() => mutSetLitSources.run(workspace, { ...settings, [key]: !on })}
          >
            <span className="inline-flex items-center gap-2">
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
