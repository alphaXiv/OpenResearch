import { useEffect, useRef, useState } from "react";
import type { Harness, HarnessSetupCommands } from "../api";
import { m } from "../paraglide/messages.js";
import { refreshHarnesses } from "../queries/settings";
import { HarnessLogo } from "./HarnessLogo";
import { CommandTerminal } from "./SshConnectTerminal";
import { initialPhase, recheckedAction, setupAction, terminalMounted, type SetupAction, type SetupPhase } from "./harnessSetupState";
import { renderNote } from "./agentNote";
import { Button, Spinner } from "./ui";

export function HarnessSetupDialog({ harness, commands, onReady, onClose }: {
  harness: Harness;
  commands: HarnessSetupCommands;
  onReady?: (harness: Harness) => void;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const cancelled = useRef(false);
  const [action, setAction] = useState<SetupAction>(() => setupAction(harness));
  const [attempt, setAttempt] = useState(0);
  const [phase, setPhase] = useState<SetupPhase>(() => initialPhase(harness, setupAction(harness)));
  // A command was approved and its terminal may connect; see `terminalMounted`.
  const [started, setStarted] = useState(phase === "running");
  // Tracked, not the mount-time prop: a re-check that clears the repair must
  // bring Retry back without reopening the dialog.
  const [needsRepair, setNeedsRepair] = useState(harness.needsConfigRepair);
  const [error, setError] = useState<string | null>(harness.needsConfigRepair ? harness.agentNote ?? null : null);
  const busy = phase === "running" || phase === "checking";
  const command = commands[action].includes("\n") ? undefined : commands[action];

  useEffect(() => {
    cancelled.current = false;
    const dialog = dialogRef.current;
    dialog?.showModal();
    return () => {
      cancelled.current = true;
      dialog?.close();
    };
  }, []);

  const verify = async () => {
    setPhase("checking");
    // Drop the previous attempt's message; this re-check replaces it.
    setError(null);
    try {
      const harnesses = await refreshHarnesses(true, true);
      if (cancelled.current) return;
      const current = harnesses.find((item) => item.id === harness.id);
      setNeedsRepair(current?.needsConfigRepair ?? false);
      if (current?.agentReady && (action !== "login" || current.authenticated)) {
        onReady?.(current);
        setPhase("success");
        if (harness.id === "antigravity" && action === "login") onClose();
      } else if (action !== "login" && current?.installed && !current.installBroken && !current.needsConfigRepair && current.authState === "needsLogin") {
        setAction("login");
        setStarted(true);
        setPhase("running");
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

  return (
    <dialog
      ref={dialogRef}
      className="m-auto w-180 max-w-[calc(100vw_-_40px)] max-h-[calc(100vh_-_40px)] overflow-y-auto rounded-xl border border-border bg-background p-5 text-text shadow-modal backdrop:bg-modal-backdrop-light"
      aria-labelledby="harness-setup-title"
      onKeyDown={(event) => {
        if (event.key === "Escape" && busy) event.preventDefault();
      }}
      onCancel={(event) => {
        event.preventDefault();
        if (!busy) onClose();
      }}
    >
      <div className="flex items-center gap-3">
        <HarnessLogo harness={harness.id} size={26} />
        <h2 id="harness-setup-title" className="m-0 text-xl font-medium">
          {m.harness_setup_title({ agent: harness.name })}
        </h2>
      </div>
      {phase === "preview" ? (
        <p className="my-4 text-sm text-subtext">
          {action === "install" ? m.harness_setup_install_preview({ agent: harness.name }) : m.harness_setup_update_preview({ agent: harness.name })}
        </p>
      ) : command && terminalMounted(phase, started) ? (
        <p className="my-4 text-sm text-subtext [&_.cmd-inline]:mx-2 [&_.cmd-inline]:gap-2">
          {renderNote(m.harness_setup_started_command({ command }))}
        </p>
      ) : null}
      {phase !== "running" && phase !== "preview" && (
        <p className="my-4 flex items-center gap-2 text-sm" role="status">
          {phase === "checking" && <Spinner />}
          {phase === "success"
            ? m.harness_setup_connected({ agent: harness.name })
            : phase === "error"
              ? m.harness_setup_failed()
              : m.harness_setup_checking()}
        </p>
      )}
      {action === "install" && commands.requiresNpm && (
        <p className="text-sm text-subtext">{m.harness_setup_requires_npm()}</p>
      )}
      {terminalMounted(phase, started) && <CommandTerminal
        key={`${action}-${attempt}`}
        path={`/api/harnesses/setup?${new URLSearchParams({ harness: harness.id, action })}`}
        label={m.harness_setup_title({ agent: harness.name })}
        command={command}
        heightClass="h-96"
        active={phase === "running"}
        awaitingApproval={phase === "preview"}
        onComplete={(value) => {
          if (typeof value !== "object" || value === null || !("type" in value) || value.type !== "complete") return false;
          void verify();
          return true;
        }}
        onError={(message) => {
          setError(message);
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
        <Button variant={phase === "success" ? "primary" : "default"} onClick={onClose}>
          {phase === "success" ? m.status_done() : busy ? m.harness_setup_cancel() : m.app_close_panel()}
        </Button>
        {phase === "preview" && (
          <Button variant="primary" onClick={() => { setStarted(true); setPhase("running"); }}>
            {m.harness_setup_approve_run()}
          </Button>
        )}
      </div>
    </dialog>
  );
}
