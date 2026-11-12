import { useWorkspace } from "../workspace";
import type { DialogContentProps } from "../dialog";
import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Terminal } from "lucide-react";
import { type Harness } from "../api";

import { m } from "../paraglide/messages.js";
import { mutSetEnvVar } from "../queries/settings";
import { HarnessLogo } from "./HarnessLogo";
import { CommandTerminal } from "./SshConnectTerminal";
import { Button, DialogContent, DialogTitle, Input, Spinner } from "./ui";

type AuthProps = {
  harness: Harness;
  remote?: boolean;
  onReady?: (harness: Harness) => void;
};

export function AcpAuthDialog({ cancel, isDisabled, ...props }: DialogContentProps<void> & AuthProps) {
  return <DialogContent className="w-140 max-h-[calc(100vh_-_40px)] overflow-y-auto p-5 text-text">
    <div className="flex items-center gap-3 mb-4">
      <HarnessLogo harness={props.harness.id} logo={props.harness.logo} size={26} />
      <DialogTitle className="m-0 text-xl font-medium">{props.harness.name}</DialogTitle>
    </div>
    <AcpAuth {...props} close={cancel} disabled={isDisabled} />
  </DialogContent>;
}

/** Signs an ACP agent in, by key or its own command. Without `close` it sits inside
 * another flow, which owns leaving; with `formId` that flow's button submits the key. */
export function AcpAuth({ harness, remote = false, onReady, close, disabled = false, formId }: AuthProps & { close?: () => void; disabled?: boolean; formId?: string }) {
  const workspace = useWorkspace();
  const { getEnvVarsQuery, refreshHarnesses } = workspace.queries;

  const cancelled = useRef(false);
  if (disabled) cancelled.current = true;
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [command, setCommand] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const environment = useQuery({ ...getEnvVarsQuery(), enabled: Boolean(harness.authEnvKey) });
  const entry = environment.data?.find((entry) => entry.key === harness.authEnvKey);
  const commands = remote ? [] : harness.logo === "zcode-acp-server" ? ["zcode login zai", "zcode login bigmodel"]
    : harness.logo === "kimi" ? ["kimi login"]
    : harness.logo === "mcode" ? ["mcode login --region global"]
    : harness.logo === "devin" ? ["devin auth login"]
    : harness.logo === "grok" ? ["grok login"]
    : harness.logo === "fx" ? ["fx login"]
    : harness.logo === "pi-acp" ? ["pi-acp --terminal-login"] : [];

  useEffect(() => {
    cancelled.current = false;
    if (!harness.authEnvKey && commands.length === 1) setCommand(commands[0]);
    return () => { cancelled.current = true; };
  }, []);

  async function verify(save = false) {
    if (cancelled.current || busy || save && (!value.trim() || entry?.inProcessEnv)) return;
    setBusy(true);
    setError(null);
    try {
      if (save && harness.authEnvKey) {
        await mutSetEnvVar.runAsHeadlessPromise(workspace, harness.authEnvKey, value.trim(), {});
        setValue("");
      }
      const current = (await refreshHarnesses(true, true)).find((item) => item.id === harness.id);
      if (cancelled.current) return;
      if (current?.agentReady && !current.authCheckFailed) { onReady?.(current); close?.(); }
      else setError(current?.agentNote ?? m.harness_setup_not_ready());
    } catch (cause) {
      if (!cancelled.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally { if (!cancelled.current) setBusy(false); }
  }

  return <>
    {harness.authEnvKey ? <form id={formId} onSubmit={(event) => { event.preventDefault(); void verify(true); }} className="flex flex-col gap-4">
      <div className="flex flex-col gap-2">
        <label className="grid gap-1.5 text-sm text-subtext">
          {m.onboarding_api_key()}
          <Input type="password" autoFocus autoComplete="new-password" value={value}
            onChange={(event) => setValue(event.target.value)} disabled={busy || entry?.inProcessEnv}
            placeholder={entry?.maskedValue ?? harness.authEnvKey} />
        </label>
        <p className="m-0 text-sm text-subtext">{entry?.inProcessEnv ? m.settings_page_overridden_by_env() : m.acp_key_environment_help()}</p>
        {harness.logo === "dsh" && <a href="https://platform.deepseek.com/api_keys" target="_blank" rel="noreferrer" className="self-start text-sm text-accent-blue-strong underline">{m.acp_get_api_key()}</a>}
      </div>
      {formId ? busy && <p role="status" className="m-0 flex items-center gap-2 text-sm text-subtext"><Spinner />{m.harness_setup_checking({ agent: harness.name })}</p> : (
        <div className="flex justify-end gap-2">
          {close && <Button type="button" onClick={() => { cancelled.current = true; close(); }}>{m.settings_page_cancel()}</Button>}
          <Button type="submit" variant="primary" disabled={busy || !value.trim() || entry?.inProcessEnv}>{busy && <Spinner />}{m.acp_save_recheck()}</Button>
        </div>
      )}
    </form> : <>
      <p className="m-0 mb-3 text-sm">{remote ? m.acp_setup_help() : m.acp_sign_in_help({ name: harness.name })}</p>
      <div className="flex flex-wrap gap-2">
        {commands.length > 1 && commands.map((item) => <Button key={item} onClick={() => { setError(null); setCommand(item); setAttempt((attempt) => attempt + 1); }} disabled={busy}>
          <Terminal size={14} />{item === "zcode login zai" ? m.acp_sign_in_zai() : item === "zcode login bigmodel" ? m.acp_sign_in_bigmodel() : m.harness_setup_login()}
        </Button>)}
        {harness.logo === "zcode-acp-server" && <a href="https://zcode.z.ai/en/docs/configuration" target="_blank" rel="noreferrer" className="self-center text-sm text-accent-blue-strong underline">{m.acp_other_provider_help()}</a>}
        {!commands.length && harness.setupUrl && <a href={harness.setupUrl} target="_blank" rel="noreferrer" className="text-sm text-accent-blue-strong underline">{m.acp_setup_docs()}</a>}
      </div>
      {command && <CommandTerminal key={`${command}:${attempt}`} path={`/api/settings/commands/run?${new URLSearchParams({ command })}`} label={command} command={command} shellAfter heightClass="h-64"
        onComplete={(message) => { if (typeof message === "object" && message !== null && "type" in message && message.type === "complete") { void verify(); return true; } return false; }}
        onError={(message) => setError(message)} />}
      <div className="flex justify-end gap-2 mt-4">
        {close && <Button onClick={() => { cancelled.current = true; close(); }}>{m.window_close()}</Button>}
        <Button onClick={() => void verify()} disabled={busy}>{busy && <Spinner />}{m.onboarding_re_check()}</Button>
      </div>
    </>}
    {error && <p className="mt-3 mb-0 text-sm text-accent-red" role="alert">{error}</p>}
  </>;
}
