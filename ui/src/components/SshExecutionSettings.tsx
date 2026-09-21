import { useEffect, useId, useState } from "react";
import { saveSshDefault, saveSshHost, testSshExecution, type SshHost, type SshSettings, type SshExecutionPreflight } from "../api";
import { m } from "../paraglide/messages.js";
import { OptionPicker } from "./ModelPicker";
import { Button, Input, showAlert } from "./ui";

export function SshDefaultHost({ settings }: { settings: SshSettings }) {
  const [saving, setSaving] = useState(false);
  return <label className="mb-4 block max-w-xl text-sm text-subtext">
    {m.ssh_default_host()}
    <OptionPicker variant="field" dropDown value={settings.defaultHost ?? ""} disabled={saving}
      choices={[
        { id: "", label: m.settings_page_not_set_pass_host_per_launch() },
        ...(settings.defaultHost && !settings.hosts.some((host) => host.host === settings.defaultHost)
          ? [{ id: settings.defaultHost, label: settings.defaultHost }] : []),
        ...settings.hosts.map((host) => ({ id: host.host, label: host.host })),
      ]}
      onSelect={(host) => {
        setSaving(true);
        void saveSshDefault(host || null).catch((error: unknown) => {
          showAlert(error instanceof Error ? error.message : String(error), "error");
        }).finally(() => setSaving(false));
      }} />
  </label>;
}

export function SshExecutionSettings({ host, connecting }: { host: SshHost; connecting: boolean }) {
  const id = useId();
  const [inContainer, setInContainer] = useState(Boolean(host.container));
  const [container, setContainer] = useState(host.container ?? "");
  const [setup, setSetup] = useState(host.setupCommand ?? "");
  const [busy, setBusy] = useState(false);
  const [test, setTest] = useState<SshExecutionPreflight | null>(null);
  useEffect(() => {
    setInContainer(Boolean(host.container));
    setContainer(host.container ?? "");
    setSetup(host.setupCommand ?? "");
    setTest(null);
  }, [host.container, host.setupCommand]);
  const reference = inContainer ? container.trim() : null;
  const dirty = reference !== (host.container ?? null) || setup !== (host.setupCommand ?? "");
  const disabled = busy || connecting;
  const invalid = inContainer && !reference;
  const error = test?.error || test?.container?.error;
  const ready = test?.reachable && test.toolsFound && (!test.container || test.container.ready);

  async function save() {
    setBusy(true);
    try {
      await saveSshHost({ host: host.host, container: reference, setupCommand: setup || null });
      showAlert(m.ssh_execution_saved(), "success");
    } catch (error) {
      showAlert(error instanceof Error ? error.message : String(error), "error");
    } finally { setBusy(false); }
  }

  async function probe() {
    setBusy(true);
    setTest(null);
    try { setTest(await testSshExecution(host.host, reference)); }
    catch (error) { showAlert(error instanceof Error ? error.message : String(error), "error"); }
    finally { setBusy(false); }
  }

  return <form className="grid max-w-xl gap-3 pb-4 ps-10 pe-2" onSubmit={(event) => { event.preventDefault(); void save(); }}>
    <label className="text-sm text-subtext">
      {m.ssh_run_in()}
      <OptionPicker variant="field" dropDown value={inContainer ? "container" : "host"} disabled={disabled}
        choices={[{ id: "host", label: m.ssh_direct_host() }, { id: "container", label: m.ssh_existing_container() }]}
        onSelect={(value) => { setInContainer(value === "container"); setTest(null); }} />
    </label>
    {inContainer && <label className="text-sm text-subtext" htmlFor={`${id}-container`}>
      {m.ssh_container_reference()}
      <Input id={`${id}-container`} value={container} disabled={disabled} required
        onChange={(event) => { setContainer(event.target.value); setTest(null); }} />
    </label>}
    <label className="text-sm text-subtext" htmlFor={`${id}-setup`}>
      {m.ssh_setup_command()}
      <textarea id={`${id}-setup`} value={setup} disabled={disabled} rows={3}
        className="block w-full rounded-md border border-border bg-background px-2.5 py-1.5 font-mono text-sm text-text outline-none focus:border-text disabled:opacity-45"
        onChange={(event) => { setSetup(event.target.value); setTest(null); }} />
    </label>
    <p className="m-0 text-sm text-subtext">{m.ssh_setup_help()}</p>
    {error && <p className="m-0 text-sm text-accent-red" role="status">{error}</p>}
    {ready && <p className="m-0 text-sm text-text" role="status">{m.ssh_execution_ready()}</p>}
    <div className="flex gap-2">
      <Button type="submit" disabled={disabled || invalid || !dirty}>{m.common_save()}</Button>
      <Button type="button" disabled={disabled || invalid} onClick={() => void probe()}>{m.ssh_test_execution()}</Button>
    </div>
  </form>;
}
