import { useEffect, useRef, useState } from "react";
import { Plus, X } from "lucide-react";
import { saveAcpHarness, testAcpHarness, type AcpDefinition } from "../api";
import { validAcpLaunch } from "../acp";
import { m } from "../paraglide/messages.js";
import { OptionPicker } from "./ModelPicker";
import { Button, IconButton, Input } from "./ui";

export const ACP_PRESETS = {
  custom: { name: "", executable: "", arguments: [], url: "https://agentclientprotocol.com/overview/agents" },
  deepseek: { name: "DeepSeek Harness", executable: "dsh", arguments: ["--profile", "acp"], url: "https://github.com/deepseek-ai/deepseek-harness" },
  kimi: { name: "Kimi Code", executable: "kimi", arguments: ["acp"], url: "https://www.kimi.com/code/docs/en/kimi-code-cli/" },
} satisfies Record<string, { name: string; executable: string; arguments: string[]; url: string }>;

export function AcpHarnessDialog({ definition, onClose, onSaved }: {
  definition?: AcpDefinition;
  onClose: () => void;
  onSaved: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [preset, setPreset] = useState<keyof typeof ACP_PRESETS>("custom");
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
    if (id !== "custom" && id !== "deepseek" && id !== "kimi") return;
    setPreset(id);
    setName(ACP_PRESETS[id].name);
    setExecutable(ACP_PRESETS[id].executable);
    setArgs(ACP_PRESETS[id].arguments);
    setConnected(null);
    setError(null);
  };
  const run = async (test: boolean) => {
    setBusy(true);
    setConnected(null);
    setError(null);
    try {
      if (test) {
        const result = await testAcpHarness(launch);
        setConnected(result.agentInfo?.version ?? "");
      } else {
        await saveAcpHarness(launch, definition?.id);
        onSaved();
        onClose();
      }
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
    } finally { setBusy(false); }
  };
  return (
    <dialog ref={dialogRef} onCancel={(event) => { event.preventDefault(); onClose(); }} aria-labelledby="acp-dialog-title"
      className="m-auto w-120 max-w-[calc(100vw_-_40px)] max-h-[calc(100vh_-_40px)] overflow-y-auto rounded-xl border border-border bg-background p-5 text-text shadow-modal backdrop:bg-modal-backdrop-light">
      <h2 id="acp-dialog-title" className="mb-4 text-lg font-medium">{definition ? m.acp_edit() : m.acp_add()}</h2>
      <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); void run(false); }}>
        <label className="flex flex-col gap-1 text-sm text-subtext">{m.acp_preset()}
          <OptionPicker variant="field" dropDown choices={[{ id: "custom", label: m.acp_custom() }, { id: "deepseek", label: "DeepSeek Harness" }, { id: "kimi", label: "Kimi Code" }]} value={preset} onSelect={changePreset} />
        </label>
        <label className="flex flex-col gap-1 text-sm text-subtext">{m.acp_name()}<Input required autoFocus value={name} onChange={(event) => setName(event.target.value)} /></label>
        <label className="flex flex-col gap-1 text-sm text-subtext">{m.acp_executable()}<Input required value={executable} onChange={(event) => { setExecutable(event.target.value); setConnected(null); }} /></label>
        <div className="flex flex-col gap-2">
          <span className="text-sm text-subtext">{m.acp_arguments()}</span>
          {args.map((argument, index) => <div key={index} className="flex items-center gap-2">
            <Input aria-label={m.acp_argument({ number: String(index + 1) })} value={argument} onChange={(event) => { setArgs(args.map((value, position) => position === index ? event.target.value : value)); setConnected(null); }} />
            <IconButton aria-label={m.chat_panel_remove()} onClick={() => { setArgs(args.filter((_, position) => position !== index)); setConnected(null); }}><X size={14} /></IconButton>
          </div>)}
          <Button type="button" size="small" className="self-start" onClick={() => { setArgs([...args, ""]); setConnected(null); }}><Plus size={14} />{m.acp_add_argument()}</Button>
        </div>
        <p className="text-sm text-text">{m.acp_setup_help()} <a href={ACP_PRESETS[preset].url} target="_blank" rel="noreferrer" className="text-primary underline">{m.acp_setup_docs()}</a></p>
        {definition && <p className="text-sm text-subtext">{m.acp_edit_help()}</p>}
        {error && <p role="alert" className="text-sm text-accent-red">{error}</p>}
        {connected !== null && <p role="status" className="text-sm text-accent-green">{m.settings_page_connected()}{connected ? ` · ${connected}` : ""}</p>}
        <div className="mt-2 flex flex-wrap justify-end gap-2">
          <Button type="button" disabled={busy || !valid} onClick={() => void run(true)}>{m.acp_test()}</Button>
          <Button type="button" onClick={onClose}>{m.settings_page_cancel()}</Button>
          <Button variant="primary" type="submit" disabled={busy || !valid}>{m.common_save()}</Button>
        </div>
      </form>
    </dialog>
  );
}
