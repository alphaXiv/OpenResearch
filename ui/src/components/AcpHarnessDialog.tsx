import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { saveAcpHarness, testAcpHarness, listAcpPresets, type AcpDefinition } from "../api";
import { workspaceKey } from "../queries/client";
import { formatAcpCommand, parseAcpCommand, validAcpLaunch } from "../acp";
import { m } from "../paraglide/messages.js";
import { OptionPicker } from "./ModelPicker";
import { Button, Input } from "./ui";

export function AcpHarnessDialog({ definition, onClose, onSaved }: {
  definition?: AcpDefinition;
  onClose: () => void;
  onSaved: (definition: AcpDefinition) => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const presets = useQuery({ queryKey: workspaceKey("getAcpPresets"), queryFn: listAcpPresets });
  const [preset, setPreset] = useState<string | null>(definition ? "custom" : null);
  const [name, setName] = useState(definition?.name ?? "");
  const [command, setCommand] = useState(definition ? formatAcpCommand(definition.executable, definition.arguments) : "");
  const [commandTouched, setCommandTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const [connected, setConnected] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const dialog = dialogRef.current;
    dialog?.showModal();
    return () => dialog?.close();
  }, []);
  const parsed = parseAcpCommand(command);
  const valid = parsed !== null && validAcpLaunch(name, parsed.executable, parsed.arguments);
  const invalidCommand = commandTouched && command.trim().length > 0 && !parsed;
  const launch = parsed ? { name, ...parsed } : null;
  const changePreset = (id: string) => {
    const selected = presets.data?.find((option) => option.id === id);
    if (id !== "custom" && !selected) return;
    setPreset(id);
    setName(selected?.name ?? "");
    setCommand(selected ? formatAcpCommand(selected.executable, selected.arguments) : "");
    setCommandTouched(false);
    setConnected(null);
    setError(null);
  };
  useEffect(() => {
    if (preset === null && presets.data?.[0]) changePreset(presets.data[0].id);
  }, [preset, presets.data]);
  const run = async (test: boolean) => {
    if (busy || !valid || !launch) return;
    setBusy(true);
    setConnected(null);
    setError(null);
    try {
      if (test) {
        const result = await testAcpHarness(launch);
        setConnected(result.agentInfo?.version ?? "");
      } else {
        if (preset !== "custom") await testAcpHarness(launch);
        const saved = await saveAcpHarness(launch, definition?.id);
        onSaved(saved);
        onClose();
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setError(preset !== "custom" && message.includes("Executable not found") ? `${name}: ${m.onboarding_not_detected()}` : message);
    } finally { setBusy(false); }
  };
  return (
    <dialog ref={dialogRef} onCancel={(event) => { event.preventDefault(); onClose(); }} aria-labelledby="acp-dialog-title"
      className="m-auto w-120 max-w-[calc(100vw_-_40px)] max-h-[calc(100vh_-_40px)] overflow-y-auto rounded-xl border border-border bg-background p-5 text-text shadow-modal backdrop:bg-modal-backdrop-light">
      <h2 id="acp-dialog-title" className="mb-4 text-lg font-medium">{definition ? m.acp_edit() : m.acp_add()}</h2>
      <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); void run(false); }}>
        <div className="text-sm leading-snug text-subtext">{m.acp_description()}</div>
        <label className="flex flex-col gap-1 text-sm text-subtext">{m.acp_preset()}
          <OptionPicker variant="field" floating dropDown disabled={busy} choices={[...(presets.data ?? []).map((option) => ({ id: option.id, label: option.name })), { id: "custom", label: m.acp_custom() }]} value={preset} onSelect={changePreset} />
        </label>
        {presets.isError && <div role="alert" className="text-sm text-accent-red">{m.harness_setup_load_failed()} <Button type="button" variant="ghost" onClick={() => void presets.refetch()}>{m.app_retry()}</Button></div>}
        {preset !== null && <details open={preset === "custom"} key={preset} className="text-sm">
        <summary className={preset === "custom" ? "hidden" : "cursor-pointer text-subtext"}>{m.acp_advanced()}</summary>
        <div className={`flex flex-col gap-3${preset === "custom" ? "" : " mt-3"}`}>
        <label className="flex flex-col gap-1 text-sm text-subtext">{m.acp_name()}<Input value={name} onChange={(event) => setName(event.target.value)} /></label>
        <div className="flex flex-col gap-1">
          <label className="flex flex-col gap-1 text-sm text-subtext">{m.acp_executable()}<Input placeholder="kimi acp" spellCheck={false} autoCapitalize="none" autoComplete="off" aria-describedby={invalidCommand ? "acp-command-error" : "acp-command-help"} aria-invalid={invalidCommand || undefined} value={command} onBlur={() => setCommandTouched(true)} onChange={(event) => { setCommand(event.target.value); setConnected(null); setError(null); }} /></label>
          <div id="acp-command-help" className="text-sm leading-snug text-subtext">{m.acp_command_help()}</div>
          {invalidCommand && <div id="acp-command-error" role="alert" className="text-sm text-accent-red">{m.acp_command_invalid()}</div>}
        </div>
        </div>
        </details>}
        <div className="text-sm"><a href={presets.data?.find((option) => option.id === preset)?.setupUrl ?? "https://agentclientprotocol.com/overview/agents"} target="_blank" rel="noreferrer" className="text-primary underline">{m.acp_setup_docs()}</a></div>
        {definition && <div className="text-sm text-subtext">{m.acp_edit_help()}</div>}
        {error && <div role="alert" className="text-sm text-accent-red">{error}</div>}
        {connected !== null && <div role="status" className="text-sm text-accent-green">{m.settings_page_connected()}{connected ? ` · ${connected}` : ""}</div>}
        <div className="mt-2 flex flex-wrap justify-end gap-2">
          {preset === "custom" && <Button type="button" disabled={busy || !valid} onClick={() => void run(true)}>{m.acp_test()}</Button>}
          <Button type="button" onClick={onClose}>{m.settings_page_cancel()}</Button>
          <Button variant="primary" type="submit" disabled={busy || !valid}>{busy ? m.settings_connecting() : definition || preset === "custom" ? m.common_save() : m.settings_connect()}</Button>
        </div>
      </form>
    </dialog>
  );
}
