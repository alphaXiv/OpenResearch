import { useId, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useWorkspace } from "../workspace";
import { m } from "../paraglide/messages.js";
import { Button, Input, LoadingRow, Spinner } from "./ui";

export function NetworkProxySection() {
  const workspace = useWorkspace();
  const id = useId();
  const settings = useQuery(workspace.queries.getNetworkProxyQuery());
  const [draft, setDraft] = useState<string | null>(null);
  const save = useMutation({
    mutationFn: (value: string) => workspace.api.setNetworkProxy(value.trim() || null),
    onSuccess: (saved) => {
      workspace.setScopedQueryData(workspace.queries.getNetworkProxyQuery().queryKey, saved);
      setDraft(null);
    },
  });
  const error = save.error ?? settings.error;
  const value = draft ?? settings.data?.proxyUrl ?? "";

  return (
    <>
      <h2>{m.settings_proxy_heading()}</h2>
      <p className="text-sm text-subtext">{m.settings_proxy_description()}</p>
      {error && <p role="alert" className="text-sm text-accent-red">{error.message}</p>}
      {!settings.data ? (
        settings.error ? <Button onClick={() => void settings.refetch()}>{m.app_retry()}</Button> : <LoadingRow><Spinner /> {m.settings_page_loading()}</LoadingRow>
      ) : (
        <form className="space-y-3" onSubmit={(event) => { event.preventDefault(); save.mutate(value); }}>
          <label className="block text-sm font-medium" htmlFor={id}>{m.settings_proxy_address()}</label>
          <Input id={id} type="url" dir="ltr" autoComplete="off" spellCheck={false}
            placeholder={m.settings_proxy_placeholder()} value={value}
            disabled={save.isPending}
            onChange={(event) => { setDraft(event.target.value); save.reset(); }} />
          <p className="text-sm text-subtext">{m.settings_proxy_default()}</p>
          <Button type="submit" disabled={save.isPending || value.trim() === (settings.data.proxyUrl ?? "")}>
            {save.isPending ? m.settings_proxy_saving() : m.common_save()}
          </Button>
          {settings.data.restartRequired && <p role="status" className="text-sm text-subtext">{m.settings_proxy_restart()}</p>}
        </form>
      )}
    </>
  );
}
