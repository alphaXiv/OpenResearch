import { useEffect, useRef, useState } from "react";
import { Plus, X } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { saveAcpHarness, testAcpHarness, listAcpPresets, type AcpDefinition } from "../api";
import { workspaceKey } from "../queries/client";
import { validAcpLaunch } from "../acp";
import { m } from "../paraglide/messages.js";
import { OptionPicker } from "./ModelPicker";
import { Button, IconButton, Input } from "./ui";

export function AcpHarnessDialog({ definition, onClose, onSaved }: {
  definition?: AcpDefinition;
  onClose: () => void;
  onSaved: (definition: AcpDefinition) => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const presets = useQuery({ queryKey: workspaceKey("getAcpPresets"), queryFn: listAcpPresets });
  const [preset, setPreset] = useState<string | null>(definition ? "custom" : null);
  const [name, setName] = useState(definition?.name ?? "");
  const [executable, setExecutable] = useState(definition?.executable ?? "");
  const [args, setArgs] = useState(definition?.arguments ?? []);
  const [busy, setBusy] = useState(false);
  const [connected, setConnected] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const dialog = dialogRef.current;
    dialog?.showModal();
    return () => dialog?.close();
  }, []);
  const valid = validAcpLaunch(name, executable, args);
  const launch = { name, executable, arguments: args };
  const changePreset = (id: string) => {
    const selected = presets.data?.find((option) => option.id === id);
    if (id !== "custom" && !selected) return;
    setPreset(id);
    setName(selected?.name ?? "");
    setExecutable(selected?.executable ?? "");
    setArgs(selected?.arguments ?? []);
    setConnected(null);
    setError(null);
  };
  useEffect(() => {
    if (preset === null && presets.data?.[0]) changePreset(presets.data[0].id);
  }, [preset, presets.data]);
  const run = async (test: boolean) => {
    if (busy || !valid) return;
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
        <label className="flex flex-col gap-1 text-sm text-subtext">{m.acp_preset()}
          <OptionPicker variant="field" floating dropDown disabled={busy} choices={[...(presets.data ?? []).map((option) => ({ id: option.id, label: option.name })), { id: "custom", label: m.acp_custom() }]} value={preset} onSelect={changePreset} />
        </label>
        {presets.isError && <p role="alert" className="text-sm text-accent-red">{m.harness_setup_load_failed()} <Button type="button" variant="ghost" onClick={() => void presets.refetch()}>{m.app_retry()}</Button></p>}
        {preset !== null && <details open={preset === "custom"} key={preset} className="text-sm">
        <summary className="cursor-pointer text-subtext">{m.acp_advanced()}</summary>
        <div className="mt-3 flex flex-col gap-3">
        <label className="flex flex-col gap-1 text-sm text-subtext">{m.acp_name()}<Input value={name} onChange={(event) => setName(event.target.value)} /></label>
        <label className="flex flex-col gap-1 text-sm text-subtext">{m.acp_executable()}<Input value={executable} onChange={(event) => { setExecutable(event.target.value); setConnected(null); }} /></label>
        <div className="flex flex-col gap-2">
          <span className="text-sm text-subtext">{m.acp_arguments()}</span>
          {args.map((argument, index) => <div key={index} className="flex items-center gap-2">
            <Input aria-label={m.acp_argument({ number: String(index + 1) })} value={argument} onChange={(event) => { setArgs(args.map((value, position) => position === index ? event.target.value : value)); setConnected(null); }} />
            <IconButton aria-label={m.chat_panel_remove()} onClick={() => { setArgs(args.filter((_, position) => position !== index)); setConnected(null); }}><X size={14} /></IconButton>
          </div>)}
          <Button type="button" size="small" className="self-start" onClick={() => { setArgs([...args, ""]); setConnected(null); }}><Plus size={14} />{m.acp_add_argument()}</Button>
        </div>
        </div>
        </details>}
        <p className="text-sm text-text">{m.acp_setup_help()} <a href={presets.data?.find((option) => option.id === preset)?.setupUrl ?? "https://agentclientprotocol.com/overview/agents"} target="_blank" rel="noreferrer" className="text-primary underline">{m.acp_setup_docs()}</a></p>
        {definition && <p className="text-sm text-subtext">{m.acp_edit_help()}</p>}
        {error && <p role="alert" className="text-sm text-accent-red">{error}</p>}
        {connected !== null && <p role="status" className="text-sm text-accent-green">{m.settings_page_connected()}{connected ? ` · ${connected}` : ""}</p>}
        <div className="mt-2 flex flex-wrap justify-end gap-2">
          {preset === "custom" && <Button type="button" disabled={busy || !valid} onClick={() => void run(true)}>{m.acp_test()}</Button>}
          <Button type="button" onClick={onClose}>{m.settings_page_cancel()}</Button>
          <Button variant="primary" type="submit" disabled={busy || !valid}>{busy ? m.settings_connecting() : definition || preset === "custom" ? m.common_save() : m.settings_connect()}</Button>
        </div>
      </form>
    </dialog>
  );
}
