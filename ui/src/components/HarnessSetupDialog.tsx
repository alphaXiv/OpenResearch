import type { DialogContentProps } from "../dialog";
import { useEffect, useRef, useState } from "react";
import type { Harness, HarnessSetupCommands } from "../api";
import { m } from "../paraglide/messages.js";
import { readHarnesses, refreshHarnesses } from "../queries/settings";
import { HarnessLogo } from "./HarnessLogo";
import { CommandTerminal } from "./SshConnectTerminal";
import { initialPhase, recheckedAction, setupAction, type SetupAction, type SetupPhase } from "./harnessSetupState";
import { renderNote } from "./agentNote";
import { Button, DialogContent, DialogTitle, Spinner } from "./ui";

export function HarnessSetupDialog({ harness, commands, onReady, onAuth, onBack, cancel, isDisabled }: DialogContentProps<void> & {
  harness: Harness;
  commands: HarnessSetupCommands;
  onReady?: (harness: Harness) => void;
  onAuth?: (harness: Harness) => void;
  onBack?: () => void;
}) {
  const cancelled = useRef(false);
  if (isDisabled) cancelled.current = true;
  const [action, setAction] = useState<SetupAction>(() => setupAction(harness));
  const [attempt, setAttempt] = useState(0);
  const [phase, setPhase] = useState<SetupPhase>(() => initialPhase(harness, setupAction(harness)));
  const [started, setStarted] = useState(phase === "running");
  // Tracked, not the mount-time prop: a re-check that clears the repair must
  // bring Retry back without reopening the dialog.
  const [needsRepair, setNeedsRepair] = useState(harness.needsConfigRepair);
  const [error, setError] = useState<string | null>(harness.needsConfigRepair ? harness.agentNote ?? null : null);
  const busy = phase === "running" || phase === "checking";
  const command = commands[action].includes("\n") ? undefined : commands[action];

  useEffect(() => {
    cancelled.current = false;
    return () => {
      cancelled.current = true;
    };
  }, []);

  // `full` re-probes every agent. After `complete` the server has published this
  // one, so a plain read suffices unless an older sweep or auth check overtook it.
  const verify = async (full = true): Promise<void> => {
    if (cancelled.current) return;
    setPhase("checking");
    // Drop the previous attempt's message; this re-check replaces it.
    setError(null);
    try {
      const harnesses = full ? await refreshHarnesses(true, true) : await readHarnesses();
      if (cancelled.current) return;
      const current = harnesses.find((item) => item.id === harness.id);
      setNeedsRepair(current?.needsConfigRepair ?? false);
      if (current?.agentReady && (action !== "login" || current.authenticated)) {
        onReady?.(current);
        cancel();
      } else if (action !== "login" && current?.installed && !current.installBroken && !current.needsConfigRepair && (current.authState === "needsLogin" || (current.id.startsWith("acp:") && !current.adapterMissing))) {
        if (current.id.startsWith("acp:") && onAuth) {
          cancel();
          onAuth(current);
          return;
        }
        setAction("login");
        setStarted(true);
        setPhase("running");
      } else if (!full) {
        return verify(true);
      } else {
        // A cleared repair frees Retry; re-point it, and drop the old action's
        // approval so nothing runs until Retry.
        const next = recheckedAction(current, action);
        if (next) {
          setAction(next);
          setStarted(false);
        }
        setError(current?.agentNote ?? m.harness_setup_not_ready());
        setPhase("error");
      }
    } catch (cause) {
      if (cancelled.current) return;
      setError(cause instanceof Error ? cause.message : String(cause));
      setPhase("error");
    }
  };

  return <DialogContent className="w-180 max-h-[calc(100vh_-_40px)] overflow-y-auto p-5 text-text">
      <div className="flex items-center gap-3">
        <HarnessLogo harness={harness.id} logo={harness.logo} size={26} />
        <DialogTitle className="m-0 text-xl font-medium">
          {harness.adapterMissing ? m.acp_install_adapter() : m.harness_setup_title({ agent: harness.name })}
        </DialogTitle>
      </div>
      {phase === "preview" ? (
        <div className="my-4">
          <p className="mb-2 text-sm text-subtext">
            {action === "install" ? m.harness_setup_install_question({ agent: harness.name }) : m.harness_setup_update_preview({ agent: harness.name })}
          </p>
          <pre
            tabIndex={0}
            dir="ltr"
            className="max-h-48 overflow-auto rounded-md border border-border bg-surface p-3 text-start font-mono text-xs text-text whitespace-pre-wrap wrap-anywhere"
          >
            <code>{commands[action]}</code>
          </pre>
        </div>
      ) : command && started ? (
        <p className="my-4 text-sm text-subtext [&_.cmd-inline]:mx-2 [&_.cmd-inline]:gap-2">
          {renderNote(m.harness_setup_started_command({ command }))}
        </p>
      ) : null}
      {phase !== "running" && phase !== "preview" && (
        <p className="my-4 flex items-center gap-2 text-sm" role="status">
          {phase === "checking" && <Spinner />}
          {phase === "error" ? m.harness_setup_failed() : m.harness_setup_checking()}
        </p>
      )}
      {action === "install" && commands.requiresNpm && (
        <p className="text-sm text-subtext">{m.harness_setup_requires_npm()}</p>
      )}
      {started && <CommandTerminal
        key={`${action}-${attempt}`}
        path={`/api/harnesses/setup?${new URLSearchParams({ harness: harness.id, action })}`}
        label={harness.adapterMissing ? m.acp_install_adapter() : m.harness_setup_title({ agent: harness.name })}
        command={command}
        heightClass="h-96"
        active={phase === "running"}
        awaitingApproval={phase === "preview"}
        onComplete={(value) => {
          if (typeof value !== "object" || value === null || !("type" in value) || value.type !== "complete") return false;
          void verify(false);
          return true;
        }}
        onError={() => {
          setError(null);
          setPhase("error");
        }}
      />}
      {error && <p role="alert" className="text-sm text-accent-red">{renderNote(error)}</p>}
      <div className="mt-5 flex justify-end gap-2">
        {phase === "error" && <Button variant="ghost" onClick={() => { setError(null); void verify(); }}>{m.onboarding_re_check()}</Button>}
        {phase === "error" && !needsRepair && <Button onClick={() => {
          setError(null);
          setStarted(true);
          setPhase("running");
          setAttempt((current) => current + 1);
        }}>{m.app_retry()}</Button>}
        <Button variant="default" onClick={() => { cancelled.current = true; cancel(); if (phase === "preview") onBack?.(); }}>
          {phase === "preview" ? m.settings_page_cancel() : busy ? m.harness_setup_cancel() : m.app_close_panel()}
        </Button>
        {phase === "preview" && (
          <Button variant="primary" onClick={() => { setStarted(true); setPhase("running"); }}>
            {m.harness_setup_approve_run()}
          </Button>
        )}
      </div>
    </DialogContent>;
}
