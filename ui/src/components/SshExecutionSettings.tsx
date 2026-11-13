import { useWorkspace } from "../workspace";
import { useId, useState } from "react";
import { useMutate } from "@clo/react-mutation";
import { type SshHost, type SshSettings } from "../api";
import { m } from "../paraglide/messages.js";
import { mutAddComputeHost, mutRemoveComputeHost, mutSaveSshDefault, mutSaveSshHost } from "../queries/settings";
import { OptionPicker } from "./ModelPicker";
import { showRemoteHostDialog } from "./RemoteHostDialog";
import { Button, Input, Spinner } from "./ui";

/** Saves a compute host from a hostname or the ssh command the user already runs. */
export function AddSshHost() {
  const workspace = useWorkspace();
  const add = useMutate(mutAddComputeHost);
  const [command, setCommand] = useState("");
  const [error, setError] = useState<string | null>(null);
  const typed = command.trim();

  return <div className="mb-4 grid gap-2">
    <form
      className="flex gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (!typed || add.isPending) return;
        setError(null);
        add.runWithOptions(workspace, typed, {
          onSuccessUi: () => setCommand(""),
          onError: (failure) => setError(failure instanceof Error ? failure.message : String(failure)),
        });
      }}
    >
      <Input
        value={command}
        onChange={(event) => {
          setCommand(event.target.value);
          setError(null);
        }}
        placeholder={m.remote_add_placeholder()}
        aria-label={m.ssh_add_host_label()}
        autoComplete="off"
        autoCapitalize="off"
        spellCheck={false}
      />
      <Button type="submit" disabled={!typed || add.isPending}>
        {add.isPending && <Spinner />}
        {m.ssh_add_host()}
      </Button>
    </form>
    {error && <p role="alert" className="m-0 text-sm text-accent-red">{error}</p>}
    <p className="m-0 text-sm text-subtext">
      {m.ssh_remote_note()}{" "}
      <button type="button" className="cursor-pointer rounded-sm text-text underline underline-offset-2 focus-visible:outline-2 focus-visible:outline-focus" onClick={() => showRemoteHostDialog(workspace.dialogs)}>
        {m.ssh_open_remote()}
      </button>
    </p>
  </div>;
}

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
  const remove = useMutate(mutRemoveComputeHost);
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
      {host.saved && <Button variant="danger" size="small" className="justify-self-start" disabled={connecting || remove.isPending}
        onClick={() => remove.run(workspace, host.host)}>
        {m.ssh_remove_host()}
      </Button>}
    </div>
  </details>;
}
