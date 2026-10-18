// arXiv and Semantic Scholar toggles in the composer's Data sources picker,
// owned by the tuandang304 fork. They use their own endpoint so upstream's
// `/api/settings/lit-sources` shape stays unchanged; see docs/fork-maintenance.md.
import { queryOptions, useMutation, useQuery } from "@tanstack/react-query";
import { get, post } from "../../api";
import { setScopedQueryData, workspaceKey } from "../../queries/client";
import { LitSourceLogo, LIT_SOURCE_NAME } from "../../components/LitSourceLogo";
import { MenuItem, SwitchIndicator } from "../../components/ui";
import { FORK_LIT_SOURCES, type ForkLitSource } from "./litSourceMeta";

export type ForkLitSourcesSettings = Record<ForkLitSource, boolean>;

export const getForkLitSources = (signal?: AbortSignal) =>
  get<ForkLitSourcesSettings>("/api/settings/fork-lit-sources", signal);

export const setForkLitSources = (body: ForkLitSourcesSettings) =>
  post<ForkLitSourcesSettings>("/api/settings/fork-lit-sources", body);

export const getForkLitSourcesQuery = () => queryOptions({
  queryKey: workspaceKey("getForkLitSources"),
  queryFn: ({ signal }) => getForkLitSources(signal),
  staleTime: 300_000,
});

export function ForkLitSourcesList() {
  const options = getForkLitSourcesQuery();
  const { data: settings } = useQuery(options);
  const mutation = useMutation({
    mutationFn: setForkLitSources,
    onSuccess: (settings) => setScopedQueryData(options.queryKey, settings),
  });
  const saving = mutation.isPending;
  if (!settings) return null;

  return FORK_LIT_SOURCES.map((key) => (
    <MenuItem
      key={key}
      type="button"
      role="switch"
      aria-checked={settings[key]}
      disabled={saving}
      onClick={() => {
        if (!saving) mutation.mutate({ ...settings, [key]: !settings[key] });
      }}
    >
      <span className="inline-flex items-center gap-[9px]">
        <LitSourceLogo source={key} size={16} decorative />
        {LIT_SOURCE_NAME[key]}
      </span>
      <SwitchIndicator checked={settings[key]} aria-hidden="true" />
    </MenuItem>
  ));
}
