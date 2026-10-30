import type { DialogContentProps } from "../dialog";
import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { saveAcpHarness, testAcpHarness, listAcpPresets, type AcpDefinition } from "../api";
import { workspaceKey } from "../queries/client";
import { formatAcpCommand, parseAcpCommand, validAcpLaunch } from "../acp";
import { m } from "../paraglide/messages.js";
import { OptionPicker } from "./ModelPicker";
import { Button, DialogContent, DialogTitle, Input } from "./ui";

export function AcpHarnessDialog({ definition, customOnly = false, cancel, confirm, isDisabled, isPending, errorMessage, onSaved }: DialogContentProps<() => Promise<void>> & {
  customOnly?: boolean;
  definition?: AcpDefinition;
  onSaved: (id: AcpDefinition["id"]) => void;
}) {
  const cancelled = useRef(false);
  if (isDisabled && !isPending) cancelled.current = true;

  useEffect(() => {
    cancelled.current = false;
    return () => { cancelled.current = true; };
  }, []);

  const presets = useQuery({ queryKey: workspaceKey("getAcpPresets"), queryFn: listAcpPresets, enabled: !customOnly });
  const [preset, setPreset] = useState<string | null>(definition || customOnly ? "custom" : null);
  const [name, setName] = useState(definition?.name ?? "");
  const [command, setCommand] = useState(definition ? formatAcpCommand(definition.executable, definition.arguments) : "");
  const [commandTouched, setCommandTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const [connected, setConnected] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
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
    if (test) setBusy(true);
    setConnected(null);
    setError(null);
    try {
      if (test) {
        const result = await testAcpHarness(launch);
        if (!cancelled.current) setConnected(result.agentInfo?.version ?? "");
      } else {
        if (preset !== "custom") {
          const result = await testAcpHarness(launch);
          const selected = presets.data?.find((option) => option.id === preset);
          if (!selected) return;
          const unchanged = selected.name === launch.name && selected.executable === launch.executable && selected.arguments.length === launch.arguments.length && selected.arguments.every((argument, index) => argument === launch.arguments[index]);
          onSaved(unchanged ? result.harnessId ?? selected.id : (await saveAcpHarness(launch)).id);
        } else {
          onSaved((await saveAcpHarness(launch, definition?.id)).id);
        }
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      const detail = preset !== "custom" && message.includes("Executable not found") ? `${name}: ${m.onboarding_not_detected()}` : message;
      if (!test) throw new Error(detail);
      if (!cancelled.current) setError(detail);
    } finally { if (test && !cancelled.current) setBusy(false); }
  };
  return <DialogContent className="w-120 max-h-[calc(100vh_-_40px)] overflow-y-auto p-5 text-text">
      <DialogTitle className="mb-4 text-lg font-medium">{definition ? m.acp_edit() : m.acp_add()}</DialogTitle>
      <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); if (!busy && valid && !isDisabled) confirm(() => run(false)); }}>
        <div className="text-sm leading-snug text-subtext">{m.acp_description()}</div>
        {!customOnly && <label className="flex flex-col gap-1 text-sm text-subtext">{m.acp_preset()}
          <OptionPicker variant="field" floating dropDown disabled={busy || isDisabled} choices={[...(presets.data ?? []).map((option) => ({ id: option.id, label: option.name })), { id: "custom", label: m.acp_custom() }]} value={preset} onSelect={changePreset} />
        </label>}
        {!customOnly && presets.isError && <div role="alert" className="text-sm text-accent-red">{m.harness_setup_load_failed()} <Button type="button" variant="ghost" onClick={() => void presets.refetch()}>{m.app_retry()}</Button></div>}
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
        <div className="text-sm"><a href={presets.data?.find((option) => option.id === preset)?.setupUrl ?? "https://agentclientprotocol.com/overview/agents"} target="_blank" rel="noreferrer" className="text-accent-blue-strong underline">{m.acp_setup_docs()}</a></div>
        {definition && <div className="text-sm text-subtext">{m.acp_edit_help()}</div>}
        {(errorMessage ?? error) && <div role="alert" className="text-sm text-accent-red">{errorMessage ?? error}</div>}
        {connected !== null && <div role="status" className="text-sm text-accent-green">{m.settings_page_connected()}{connected ? ` · ${connected}` : ""}</div>}
        <div className="mt-2 flex flex-wrap justify-end gap-2">
          {preset === "custom" && <Button type="button" disabled={busy || isDisabled || !valid} onClick={() => void run(true)}>{m.acp_test()}</Button>}
          <Button type="button" onClick={cancel} disabled={isDisabled}>{m.settings_page_cancel()}</Button>
          <Button variant="primary" type="submit" disabled={busy || isDisabled || !valid}>{busy || isPending ? m.settings_connecting() : definition || preset === "custom" ? m.common_save() : m.settings_connect()}</Button>
        </div>
      </form>
    </DialogContent>;
}
