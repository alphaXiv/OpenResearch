import { useWorkspace } from "../workspace";
import { useId } from "react";
import { useMutate } from "@clo/react-mutation";
import { type SshHost, type SshSettings } from "../api";
import { m } from "../paraglide/messages.js";
import { mutSaveSshDefault, mutSaveSshHost } from "../queries/settings";
import { OptionPicker } from "./ModelPicker";
import { Input } from "./ui";

export function SshDefaultHost({ settings }: { settings: SshSettings }) {
  const workspace = useWorkspace();

  return <label className="inline-flex max-w-full flex-wrap items-center gap-2 text-sm text-subtext">
    {m.ssh_default_host()}
    <span className="inline-block w-max max-w-full">
      <OptionPicker variant="field" dropDown value={settings.defaultHost ?? ""}
        choices={[
          { id: "", label: m.settings_page_not_set_pass_host_per_launch() },
          ...(settings.defaultHost && !settings.hosts.some((host) => host.host === settings.defaultHost)
            ? [{ id: settings.defaultHost, label: settings.defaultHost }] : []),
          ...settings.hosts.map((host) => ({ id: host.host, label: host.host })),
        ]}
        onSelect={(host) => mutSaveSshDefault.run(workspace, host || null)} />
    </span>
  </label>;
}

export function SshExecutionSettings({ host, connecting, reference, onChange }: {
  host: SshHost;
  connecting: boolean;
  reference: string | null;
  onChange: (reference: string | null) => void;
}) {
  const workspace = useWorkspace();

  const id = useId();
  const save = useMutate(mutSaveSshHost);
  const failure = save.error;
  function change(value: string | null) {
    onChange(value);
    save.clear();
    if (value !== null && !value.trim()) return;
    save.run(workspace, host.host, value?.trim() ?? null);
  }

  return <details className="pb-3">
    <summary className="w-fit cursor-pointer rounded-sm text-sm text-subtext focus-visible:outline-2 focus-visible:outline-focus">{m.new_project_advanced()}</summary>
    <div className="mt-3 grid max-w-xl gap-3">
      <label className="text-sm text-subtext">
        {m.ssh_run_in()}
        <OptionPicker variant="field" dropDown value={reference !== null ? "container" : "host"} disabled={connecting}
          choices={[{ id: "host", label: m.ssh_direct_host() }, { id: "container", label: m.ssh_existing_container() }]}
          onSelect={(value) => change(value === "container" ? "" : null)} />
      </label>
      {reference !== null && <label className="text-sm text-subtext" htmlFor={`${id}-container`}>
        {m.ssh_container_reference()}
        <Input id={`${id}-container`} value={reference} disabled={connecting} required
          onChange={(event) => change(event.target.value)} />
      </label>}
      {failure != null && <p className="m-0 text-sm text-accent-red" role="status">{failure instanceof Error ? failure.message : String(failure)}</p>}
    </div>
  </details>;
}
