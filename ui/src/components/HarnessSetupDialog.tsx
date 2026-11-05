import { useWorkspace } from "../workspace";
import type { DialogContentProps } from "../dialog";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { ChevronDown } from "lucide-react";
import type { Harness, HarnessSetupCommands } from "../api";
import { fmtNumber } from "../i18n";
import { m } from "../paraglide/messages.js";
import { selectionHarness } from "../modelSelection";
import { isRecord } from "../workspaceState";

import { HarnessLogo } from "./HarnessLogo";
import { ProgressBar } from "./ProgressBar";
import { CommandTerminal } from "./SshConnectTerminal";
import { initialPhase, recheckedAction, setupAction, type SetupAction, type SetupPhase } from "./harnessSetupState";
import { renderNote } from "./agentNote";
import { Button, DialogContent, DialogTitle, Input, Spinner } from "./ui";
import { cn } from "./ui/cn";

type SetupProps = {
  harness: Harness;
  commands: HarnessSetupCommands;
  /** Runs this instead of what the agent's state asks for, such as an upgrade the user chose. */
  action?: SetupAction;
  /** Claude Code or Codex: the account OpenResearch keeps that a sign-in is for. */
  profile?: string;
  onReady?: (harness: Harness) => void;
  onAuth?: (harness: Harness) => void;
};

/** These sign-ins send their page's address, so their terminal can stay out of the way. */
const PAGE_SIGN_INS = new Set(["claude-code", "codex", "cursor"]);

export function HarnessSetupDialog({ cancel, isDisabled, onBack, ...props }: DialogContentProps<void> & SetupProps & { onBack?: () => void }) {
  const name = props.harness.profile?.name ?? props.harness.label?.name ?? props.harness.name;
  return <DialogContent className="w-180 max-h-[calc(100vh_-_40px)] overflow-y-auto p-5 text-text">
      <div className="mb-4 flex items-center gap-3">
        <HarnessLogo harness={props.harness.id} logo={props.harness.logo} size={26} />
        <DialogTitle className="m-0 text-xl font-medium">
          {props.harness.adapterMissing ? m.acp_install_adapter() : props.action === "login" ? m.add_agent_sign_in_title({ name }) : m.harness_setup_title({ agent: name })}
        </DialogTitle>
      </div>
      <HarnessSetup {...props} disabled={isDisabled} close={cancel} onBack={onBack} />
    </DialogContent>;
}

/** Installs, updates or signs in to an agent. Without `close` it sits inside
 * another flow, which owns leaving. */
export function HarnessSetup({ harness, commands, action: forced, profile, onReady, onAuth, onBack, close, disabled = false }: SetupProps & {
  close?: () => void;
  onBack?: () => void;
  disabled?: boolean;
}) {
  const { refreshHarnesses, readHarnesses } = useWorkspace().queries;

  const cancelled = useRef(false);
  if (disabled) cancelled.current = true;
  const [action, setAction] = useState<SetupAction>(() => forced ?? setupAction(harness));
  const [attempt, setAttempt] = useState(0);
  const [phase, setPhase] = useState<SetupPhase>(() => initialPhase(harness, action, Boolean(commands.managedFrom)));
  const [started, setStarted] = useState(phase === "running");
  // Tracked, not the mount-time prop: a re-check that clears the repair must
  // bring Retry back without reopening the dialog.
  const [needsRepair, setNeedsRepair] = useState(harness.needsConfigRepair);
  const [error, setError] = useState<string | null>(harness.needsConfigRepair ? harness.agentNote ?? null : null);
  const busy = phase === "running" || phase === "checking";
  const shown = action === "upgrade" ? undefined : commands[action];
  const command = shown && !shown.includes("\n") ? shown : undefined;
  // OpenResearch downloads its own copy: no command runs, so there's only progress to show.
  const managed = action === "install" && Boolean(commands.managedFrom);
  const pageSignIn = action === "login" && PAGE_SIGN_INS.has(harness.id);
  const setupPath = `/api/harnesses/setup?${new URLSearchParams({ harness: harness.id, action, ...(profile && action === "login" ? { profile } : {}) })}`;
  const name = harness.profile?.name ?? harness.label?.name ?? harness.name;

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
      // An account's own entry decides its sign-in; the agent's says only that it's installed.
      const current = selectionHarness(harnesses, { harness: harness.id, profile });
      setNeedsRepair(current?.needsConfigRepair ?? false);
      if (current?.agentReady && (action !== "login" || current.authenticated)) {
        onReady?.(current);
        close?.();
      } else if (action !== "login" && current?.installed && !current.installBroken && !current.needsConfigRepair && (current.authState === "needsLogin" || (current.id.startsWith("acp:") && !current.adapterMissing))) {
        if (current.id.startsWith("acp:") && onAuth) {
          close?.();
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
  const fail = (message: string) => {
    setError(message);
    setPhase("error");
  };

  return <>
      {phase === "preview" && (
        <div className="my-4">
          <p className="mb-2 text-sm text-subtext">
            {action === "install" ? m.harness_setup_install_question({ agent: harness.name }) : m.harness_setup_update_preview({ agent: harness.name })}
          </p>
          <pre
            tabIndex={0}
            dir="ltr"
            className="max-h-48 overflow-auto rounded-md border border-border bg-surface p-3 text-start font-mono text-xs text-text whitespace-pre-wrap wrap-anywhere"
          >
            <code>{shown}</code>
          </pre>
          {action === "update" && commands.managedFrom && (
            <p className="mt-2 text-sm text-subtext">{m.harness_setup_update_fallback({ agent: harness.name })}</p>
          )}
        </div>
      )}
      {started && (managed ? (
        <InstallProgress key={attempt} path={setupPath} agent={harness.name} running={phase === "running"} onComplete={() => void verify(false)} onError={fail} />
      ) : (
        <SetupRun
          key={`${action}-${attempt}`}
          path={setupPath}
          label={harness.adapterMissing ? m.acp_install_adapter() : m.harness_setup_title({ agent: name })}
          command={command}
          running={phase === "running"}
          signIn={action === "login" ? { agent: harness.name, page: pageSignIn, code: harness.id === "claude-code" } : undefined}
          status={action === "install" ? m.harness_setup_installing({ agent: harness.name }) : m.harness_setup_updating({ agent: harness.name })}
          // OpenCode's own upgrade can ask before it installs, and a sign-in without a page happens in the terminal.
          expanded={action === "upgrade" || (action === "login" && !pageSignIn)}
          onComplete={() => void verify(false)}
          onError={fail}
        />
      ))}
      {phase !== "running" && phase !== "preview" && !(phase === "error" && error) && (
        <p className="my-4 flex items-center gap-2 text-sm" role="status">
          {phase === "checking" && <Spinner />}
          {phase === "error" ? m.harness_setup_failed() : action === "login" ? m.harness_setup_checking_sign_in() : m.harness_setup_checking({ agent: harness.name })}
        </p>
      )}
      {action === "install" && commands.requiresNpm && (
        <p className="text-sm text-subtext">{m.harness_setup_requires_npm()}</p>
      )}
      {error && <p role="alert" className="text-sm text-accent-red">{renderNote(error)}</p>}
      <div className={cn("mt-5 flex gap-2 empty:hidden", close ? "justify-end" : "justify-start")}>
        {phase === "error" && !pageSignIn && <Button variant="ghost" onClick={() => { setError(null); void verify(); }}>{m.onboarding_re_check()}</Button>}
        {phase === "error" && !needsRepair && <Button onClick={() => {
          setError(null);
          setStarted(true);
          setPhase("running");
          setAttempt((current) => current + 1);
        }}>{m.app_retry()}</Button>}
        {close && <Button variant="default" onClick={() => { cancelled.current = true; close(); if (phase === "preview") onBack?.(); }}>
          {phase === "preview" ? m.settings_page_cancel() : busy ? m.harness_setup_cancel() : m.app_close_panel()}
        </Button>}
        {phase === "preview" && (
          <Button variant="primary" onClick={() => { setStarted(true); setPhase("running"); }}>{m.harness_setup_approve_run()}</Button>
        )}
      </div>
    </>;
}

type Progress = { received: number; total: number | null };

/** `type: progress` frames, which managed downloads send beside their text. */
function progressFrame(value: unknown): Progress | null {
  if (!isRecord(value) || value.type !== "progress" || typeof value.received !== "number") return null;
  return { received: value.received, total: typeof value.total === "number" ? value.total : null };
}

function DownloadBar({ progress }: { progress: Progress | null }) {
  const megabytes = (bytes: number) => fmtNumber(Math.round(bytes / 1e6));
  const label = !progress ? undefined
    : progress.total === null ? (progress.received < 5e5 ? undefined : m.harness_setup_downloaded({ received: megabytes(progress.received) }))
      : progress.received >= progress.total ? m.common_checking()
        : m.harness_setup_download_size({ received: megabytes(progress.received), total: megabytes(progress.total) });
  return <ProgressBar value={progress?.total ? Math.min(progress.received, progress.total) : undefined} max={progress?.total ?? undefined} label={label} />;
}

/** A copy OpenResearch downloads itself: no command runs, so there is no terminal, only progress. */
function InstallProgress({ path, agent, running, onComplete, onError }: {
  path: string;
  agent: string;
  running: boolean;
  onComplete: () => void;
  onError: (message: string) => void;
}) {
  const { socketUrl } = useWorkspace();
  const [progress, setProgress] = useState<Progress | null>(null);
  const events = useRef({ onComplete, onError });
  events.current = { onComplete, onError };

  useEffect(() => {
    const socket = new WebSocket(socketUrl(path));
    let ended = false;
    const end = (error?: string) => {
      ended = true;
      socket.close();
      if (error === undefined) events.current.onComplete();
      else events.current.onError(error);
    };
    // Binary frames repeat the progress as text for old dashboards.
    socket.onmessage = (event) => {
      if (typeof event.data !== "string") return;
      let value: unknown;
      try { value = JSON.parse(event.data); } catch { return; }
      const next = progressFrame(value);
      if (next) setProgress(next);
      else if (isRecord(value) && value.type === "complete") end();
      else if (isRecord(value) && value.type === "error") end(typeof value.error === "string" ? value.error : m.harness_setup_failed());
    };
    socket.onclose = () => { if (!ended) end(m.settings_terminal_closed()); };
    return () => {
      ended = true;
      socket.onmessage = null;
      socket.onclose = null;
      socket.close();
    };
  }, [path]);

  if (!running) return null;
  return (
    <div className="mt-1">
      <p role="status" className="m-0 text-sm">{m.harness_setup_installing({ agent })}</p>
      <DownloadBar progress={progress} />
    </div>
  );
}

/** "Open the sign-in page and …", with the page linked inside the sentence. */
export function SignInSteps({ url, message }: { url: string; message: (inputs: { link: string }) => string }) {
  const [before, after] = message({ link: "\u0000" }).split("\u0000");
  return (
    <p className="m-0 text-sm">
      {before}
      <a href={url} target="_blank" rel="noreferrer" className="text-accent-blue-strong underline">{m.harness_setup_signin_page()}</a>
      {after}
    </p>
  );
}

/** A command in a terminal kept out of the way: a status line, the terminal behind a
 * disclosure that opens itself on failure, and for a sign-in its steps and code. The
 * terminal stays mounted and sized while closed, so its command keeps running. */
function SetupRun({ path, label, command, running, signIn, status, expanded, onComplete, onError }: {
  path: string;
  label: string;
  command?: string;
  running: boolean;
  /** `page` when the agent prints its sign-in page's address, else the sign-in happens in the
   * terminal; `code` when that page may hand back a code to paste. */
  signIn?: { agent: string; page: boolean; code: boolean };
  /** What runs, shown while it does; a sign-in says its own. */
  status: string;
  /** The terminal starts open, for a command that may ask something. */
  expanded: boolean;
  onComplete: () => void;
  onError: (message: string) => void;
}) {
  const terminalId = useId();
  const [open, setOpen] = useState(expanded);
  const [url, setUrl] = useState<string | null>(null);
  const [code, setCode] = useState("");
  const [sent, setSent] = useState(false);
  const [progress, setProgress] = useState<Progress | null>(null);
  const sendRef = useRef<((message: object) => void) | null>(null);

  const send = () => {
    if (code.trim().length < 4 || sent || !sendRef.current) return;
    sendRef.current({ type: "code", code: code.trim() });
    setSent(true);
  };
  const line: ReactNode = !running ? null
    : !signIn ? status
      : signIn.page && !url ? m.harness_setup_signin_starting()
        : sent ? m.harness_setup_signin_checking_code()
          : m.harness_setup_signin_waiting();

  return (
    <div className="mt-1 flex flex-col gap-3">
      {running && signIn && !signIn.page && <p className="m-0 text-sm">{m.acp_sign_in_help({ name: signIn.agent })}</p>}
      {running && url && signIn?.page && <SignInSteps url={url} message={signIn.code ? m.harness_setup_signin_steps_code : m.harness_setup_signin_steps} />}
      {running && url && signIn?.code && (
        <form className="grid gap-1.5" onSubmit={(event) => { event.preventDefault(); send(); }}>
          <label htmlFor={`${terminalId}-code`} className="text-sm text-subtext">{m.harness_setup_signin_code()}</label>
          <div className="flex gap-2">
            <Input
              id={`${terminalId}-code`}
              type="password"
              autoComplete="off"
              spellCheck={false}
              className="font-mono"
              value={code}
              onChange={(event) => { setCode(event.target.value); setSent(false); }}
            />
            <Button type="submit" disabled={sent || code.trim().length < 4}>{m.chat_panel_send()}</Button>
          </div>
        </form>
      )}
      {line && (
        <p role="status" className="m-0 flex min-h-5 items-center gap-2 text-sm text-subtext">
          <Spinner />{line}
        </p>
      )}
      {running && progress && <DownloadBar progress={progress} />}
      <div>
        <button
          type="button"
          aria-expanded={open}
          aria-controls={terminalId}
          onClick={() => setOpen((current) => !current)}
          className="flex items-center gap-1.5 rounded-md py-0.5 text-sm text-subtext outline-none hover:text-text focus-visible:outline-2 focus-visible:outline-focus"
        >
          <ChevronDown size={14} className={cn("transition-transform", !open && "-rotate-90")} aria-hidden="true" />
          {m.harness_setup_terminal()}
        </button>
        <div id={terminalId} inert={!open} className={open ? "mt-3" : "h-0 overflow-hidden"}>
          <CommandTerminal
            path={path}
            label={label}
            command={command}
            heightClass="h-72"
            active={running && open}
            sendRef={sendRef}
            onComplete={(value) => {
              const next = progressFrame(value);
              if (next) setProgress(next);
              if (isRecord(value) && value.type === "signin" && typeof value.url === "string") setUrl(value.url);
              if (!isRecord(value) || value.type !== "complete") return false;
              onComplete();
              return true;
            }}
            onError={(message) => {
              // A turned-down code is told above; the terminal opens for other failures.
              if (!signIn?.page) setOpen(true);
              onError(message);
            }}
          />
        </div>
      </div>
    </div>
  );
}
