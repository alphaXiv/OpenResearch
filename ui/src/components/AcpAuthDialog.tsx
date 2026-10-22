import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Terminal } from "lucide-react";
import { setEnvVar, type Harness } from "../api";
import { getEnvVarsQuery, refreshHarnesses } from "../queries/settings";
import { queryClient } from "../queries/client";
import { m } from "../paraglide/messages.js";
import { HarnessLogo } from "./HarnessLogo";
import { CommandTerminal } from "./SshConnectTerminal";
import { Button, Input, Spinner } from "./ui";

export function AcpAuthDialog({ harness, remote = false, onReady, onClose }: {
  harness: Harness;
  remote?: boolean;
  onReady?: (harness: Harness) => void;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const cancelled = useRef(false);
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [command, setCommand] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const envOptions = getEnvVarsQuery();
  const environment = useQuery({ ...envOptions, enabled: Boolean(harness.authEnvKey) });
  const entry = environment.data?.find((entry) => entry.key === harness.authEnvKey);
  const commands = remote ? [] : harness.logo === "zcode-acp-server" ? ["zcode login zai", "zcode login bigmodel"]
    : harness.logo === "kimi" ? ["kimi login"]
    : harness.logo === "mcode" ? ["mcode login --region global"]
    : harness.logo === "devin" ? ["devin auth login"]
    : harness.logo === "grok" ? ["grok login"]
    : harness.logo === "pi-acp" ? ["pi"] : [];

  useEffect(() => {
    cancelled.current = false;
    const dialog = dialogRef.current;
    dialog?.showModal();
    dialog?.querySelector<HTMLInputElement>('input[type="password"]')?.focus();
    return () => { cancelled.current = true; dialog?.close(); };
  }, []);

  async function verify(save = false) {
    if (busy || save && (!value.trim() || entry?.inProcessEnv)) return;
    setBusy(true);
    setError(null);
    try {
      if (save && harness.authEnvKey) {
        await setEnvVar(harness.authEnvKey, value.trim());
        setValue("");
        void queryClient.invalidateQueries({ queryKey: envOptions.queryKey });
      }
      const current = (await refreshHarnesses(true, true)).find((item) => item.id === harness.id);
      if (cancelled.current) return;
      if (current?.agentReady && !current.authCheckFailed) { onReady?.(current); onClose(); }
      else setError(current?.agentNote ?? m.harness_setup_not_ready());
    } catch (cause) {
      if (!cancelled.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally { if (!cancelled.current) setBusy(false); }
  }

  return <dialog ref={dialogRef} aria-labelledby="acp-auth-title"
    className="m-auto w-140 max-w-[calc(100vw_-_40px)] max-h-[calc(100vh_-_40px)] overflow-y-auto rounded-xl border border-border bg-background p-5 text-text shadow-modal backdrop:bg-modal-backdrop-light"
    onCancel={(event) => { event.preventDefault(); onClose(); }}>
    <div className="flex items-center gap-3 mb-4">
      <HarnessLogo harness={harness.id} logo={harness.logo} size={26} />
      <h2 id="acp-auth-title" className="m-0 text-xl font-medium">{harness.name}</h2>
    </div>
    {harness.authEnvKey ? <form onSubmit={(event) => { event.preventDefault(); void verify(true); }} className="flex flex-col gap-3">
      <p className="m-0 text-sm">{m.acp_key_environment_help()}</p>
      {harness.logo === "dsh" && <a href="https://platform.deepseek.com/api_keys" target="_blank" rel="noreferrer" className="text-sm text-primary underline">{m.acp_get_api_key()}</a>}
      <label className="flex flex-col gap-1 text-sm">
        {m.onboarding_api_key()}
        <Input type="password" autoFocus autoComplete="new-password" value={value}
          onChange={(event) => setValue(event.target.value)} disabled={busy || entry?.inProcessEnv}
          placeholder={entry?.maskedValue ?? harness.authEnvKey} />
      </label>
      {entry?.inProcessEnv && <p className="m-0 text-sm text-subtext">{m.settings_page_overridden_by_env()}</p>}
      <div className="flex justify-end gap-2">
        <Button type="button" onClick={onClose}>{m.settings_page_cancel()}</Button>
        <Button type="submit" variant="primary" disabled={busy || !value.trim() || entry?.inProcessEnv}>{busy && <Spinner />}{m.acp_save_recheck()}</Button>
      </div>
    </form> : <>
      <p className="m-0 mb-3 text-sm">{remote ? m.acp_setup_help() : harness.logo === "pi-acp" ? m.acp_pi_login_help() : m.acp_sign_in_help({ name: harness.name })}</p>
      <div className="flex flex-wrap gap-2">
        {commands.map((item) => <Button key={item} onClick={() => { setError(null); setCommand(item); setAttempt((attempt) => attempt + 1); }} disabled={busy}>
          <Terminal size={14} />{item === "zcode login zai" ? m.acp_sign_in_zai() : item === "zcode login bigmodel" ? m.acp_sign_in_bigmodel() : m.harness_setup_login()}
        </Button>)}
        {harness.logo === "zcode-acp-server" && <a href="https://zcode.z.ai/en/docs/configuration" target="_blank" rel="noreferrer" className="self-center text-sm text-primary underline">{m.acp_other_provider_help()}</a>}
        {!commands.length && harness.setupUrl && <a href={harness.setupUrl} target="_blank" rel="noreferrer" className="text-sm text-primary underline">{m.acp_setup_docs()}</a>}
      </div>
      {command && <CommandTerminal key={`${command}:${attempt}`} path={`/api/settings/commands/run?${new URLSearchParams({ command })}`} label={command} command={command} shellAfter heightClass="h-64"
        onComplete={(message) => { if (typeof message === "object" && message !== null && "type" in message && message.type === "complete") { void verify(); return true; } return false; }}
        onError={(message) => setError(message)} />}
      <div className="flex justify-end gap-2 mt-4">
        <Button onClick={onClose}>{m.window_close()}</Button>
        <Button onClick={() => void verify()} disabled={busy}>{busy && <Spinner />}{m.onboarding_re_check()}</Button>
      </div>
    </>}
    {error && <p className="mt-3 mb-0 text-sm text-accent-red" role="alert">{error}</p>}
  </dialog>;
}
