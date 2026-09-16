import { useEffect, useRef, useState } from "react";
import { Check, X } from "lucide-react";
import type { SlurmPreflight, SshPreflight } from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { Spinner } from "./ui/Spinner";
import { mountTerminal, type TerminalPalette } from "./terminal";

export type SshConnectResult =
  | { backend: "ssh"; result: SshPreflight }
  | { backend: "slurm"; result: SlurmPreflight };

const TERMINAL_CLASS_NAME = "overflow-hidden rounded-md bg-terminal p-2";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

function isSshPreflight(value: unknown): value is SshPreflight {
  return (
    isRecord(value) &&
    typeof value.reachable === "boolean" &&
    typeof value.toolsFound === "boolean" &&
    (value.missingTools === undefined || isStringArray(value.missingTools)) &&
    (value.error === null || typeof value.error === "string") &&
    typeof value.testedAt === "number"
  );
}

function isSlurmPreflight(value: unknown): value is SlurmPreflight {
  return (
    isRecord(value) &&
    typeof value.reachable === "boolean" &&
    typeof value.slurmFound === "boolean" &&
    typeof value.toolsFound === "boolean" &&
    isStringArray(value.partitions) &&
    (value.error === null || typeof value.error === "string")
  );
}

function connectionResult(value: unknown): SshConnectResult | null {
  if (!isRecord(value) || value.type !== "complete") return null;
  if (value.backend === "ssh" && isSshPreflight(value.result)) {
    return { backend: "ssh", result: value.result };
  }
  if (value.backend === "slurm" && isSlurmPreflight(value.result)) {
    return { backend: "slurm", result: value.result };
  }
  return null;
}

function serverError(value: unknown): string | null {
  return isRecord(value) && value.type === "error" && typeof value.error === "string"
    ? value.error
    : null;
}

export function SshConnectTerminal({
  host,
  backend,
  path = "/api/settings/ssh/connect",
  active = true,
  onComplete,
  onError,
}: {
  host: string;
  backend: "ssh" | "slurm";
  path?: string;
  active?: boolean;
  onComplete: (result: SshConnectResult) => void;
  onError?: (error: string) => void;
}) {
  const query = new URLSearchParams({ host, backend });
  return <CommandTerminal
    path={`${path}?${query}`}
    label={m.settings_ssh_connection_terminal({ host: ltr(host) })}
    active={active}
    onError={onError}
    onComplete={(value) => {
      const result = connectionResult(value);
      if (!result) return false;
      onComplete(result);
      return true;
    }}
  />;
}

export function OpenResearchSetupTerminal({ login, onComplete, onError }: {
  login: boolean;
  onComplete: () => void;
  onError: (error: string) => void;
}) {
  return <CommandTerminal
    path={login ? "/api/settings/openresearch/login" : "/api/settings/openresearch/ssh-key"}
    label={login ? "orx login" : "orx ssh-key add"}
    heightClass="h-80"
    onError={onError}
    onComplete={(value) => {
      if (!isRecord(value) || value.type !== "complete") return false;
      onComplete();
      return true;
    }}
  />;
}

type CommandStatus = "running" | "done" | "failed" | "closed";

/** Runs a settings command (`claude auth login`, `gh auth login`) in place, so
 * the user never has to copy it into a terminal of their own. Framed like an
 * app window; after the command exits the same terminal continues as the
 * user's shell, so a follow-up command needs no copy-paste either. */
export function SettingsCommandTerminal({ path, label, onComplete, onError, onClose }: {
  path: string;
  label: string;
  onComplete: () => void;
  onError: (error: string) => void;
  onClose: () => void;
}) {
  const [status, setStatus] = useState<CommandStatus>("running");
  return (
    <div className="mt-4 overflow-hidden rounded-lg border border-border bg-terminal-app">
      <div className="flex h-9 items-center gap-3 border-b border-b-border-variant bg-surface ps-3 pe-1.5">
        <code dir="ltr" className="min-w-0 flex-1 truncate font-mono text-xs text-subtext">
          {label}
        </code>
        <span role="status" className="flex shrink-0 items-center gap-1.5 text-xs text-subtext">
          {status === "running" ? (
            <><Spinner className="border-t-accent-amber" /> {m.settings_command_terminal_running()}</>
          ) : status === "done" ? (
            <><Check size={13} strokeWidth={2.5} className="text-accent-green" /> {m.settings_command_terminal_done()}</>
          ) : status === "failed" ? (
            <><X size={13} strokeWidth={2.5} className="text-accent-red" /> {m.settings_command_terminal_failed()}</>
          ) : (
            m.settings_command_terminal_closed()
          )}
        </span>
        <button
          type="button"
          onClick={onClose}
          aria-label={m.settings_command_terminal_close()}
          title={m.settings_command_terminal_close()}
          className="ms-1 inline-flex h-6 w-6 shrink-0 cursor-pointer items-center justify-center rounded-md border-0 bg-transparent text-muted [&:hover]:bg-highlight [&:hover]:text-text"
        >
          <X size={14} />
        </button>
      </div>
      <CommandTerminal
        path={path}
        label={label}
        heightClass="h-80"
        frame="bare"
        palette="app"
        shellAfter
        onError={(error) => {
          setStatus("failed");
          onError(error);
        }}
        onComplete={(value) => {
          if (!isRecord(value) || value.type !== "complete") return false;
          setStatus("done");
          onComplete();
          return true;
        }}
        onClosed={() => setStatus("closed")}
      />
    </div>
  );
}

function CommandTerminal({ path, label, heightClass = "h-40", frame = "card", palette = "dark", shellAfter = false, active = true, onComplete, onError, onClosed }: {
  path: string;
  label: string;
  heightClass?: string;
  /** `bare` drops the rounded card so a caller can supply its own chrome. */
  frame?: "card" | "bare";
  palette?: TerminalPalette;
  /** The server keeps the session open as a shell once the command reports
   * completion or failure, so the socket stays up and input stays enabled. */
  shellAfter?: boolean;
  active?: boolean;
  onComplete: (value: unknown) => boolean;
  onError?: (error: string) => void;
  /** The session ended after completion (the follow-up shell exited). */
  onClosed?: () => void;
}) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<ReturnType<typeof mountTerminal>["terminal"] | null>(null);
  const completeRef = useRef(onComplete);
  const errorRef = useRef(onError);
  const closedRef = useRef(onClosed);
  const [error, setError] = useState<string | null>(null);
  completeRef.current = onComplete;
  errorRef.current = onError;
  closedRef.current = onClosed;

  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const { terminal, dispose } = mountTerminal(wrap, false, true, palette);
    terminalRef.current = terminal;
    terminal.focus();
    const protocol = location.protocol === "https:" ? "wss:" : "ws:";
    const url = new URL(path, `${protocol}//${location.host}`);
    const socket = new WebSocket(url);
    socket.binaryType = "arraybuffer";
    let completed = false;
    let failed = false;
    let receivedOutput = false;
    const fail = (message: string, sessionEnded: boolean) => {
      if (failed) return;
      failed = true;
      if (!receivedOutput) terminal.writeln(message);
      errorRef.current?.(message);
      // A server-reported failure is followed by the shell, which still takes input.
      if (shellAfter && !sessionEnded) return;
      terminal.options.disableStdin = true;
      terminal.blur();
      setError(message);
    };

    const input = terminal.onData((data) => {
      if (socket.readyState === WebSocket.OPEN) socket.send(new TextEncoder().encode(data));
    });
    const resize = terminal.onResize(({ cols, rows }) => {
      if (socket.readyState === WebSocket.OPEN) {
        socket.send(JSON.stringify({ type: "resize", cols, rows }));
      }
    });
    socket.onopen = () => {
      socket.send(JSON.stringify({ type: "resize", cols: terminal.cols, rows: terminal.rows }));
    };
    socket.onmessage = (event) => {
      if (event.data instanceof ArrayBuffer) {
        receivedOutput = true;
        terminal.write(new Uint8Array(event.data));
        return;
      }
      if (typeof event.data !== "string") return;
      let value: unknown;
      try {
        value = JSON.parse(event.data);
      } catch {
        return;
      }
      if (completeRef.current(value)) {
        completed = true;
        if (!shellAfter) socket.close();
        return;
      }
      const message = serverError(value);
      if (message) fail(message, false);
    };
    socket.onerror = () => fail(m.settings_terminal_closed(), true);
    socket.onclose = () => {
      if (!completed && !failed) {
        fail(m.settings_terminal_closed(), true);
        return;
      }
      if (shellAfter) {
        terminal.options.disableStdin = true;
        terminal.blur();
        closedRef.current?.();
      }
    };

    return () => {
      socket.onopen = null;
      socket.onmessage = null;
      socket.onerror = null;
      socket.onclose = null;
      input.dispose();
      resize.dispose();
      socket.close();
      terminalRef.current = null;
      dispose();
    };
  }, [path, palette, shellAfter]);

  useEffect(() => {
    const terminal = terminalRef.current;
    if (!terminal) return;
    terminal.options.disableStdin = !active || error !== null;
    if (active && error === null) terminal.focus();
    else terminal.blur();
  }, [active, error]);

  return (
    <div className={frame === "card" ? "mt-3" : undefined}>
      <div
        className={`${heightClass} ${frame === "card" ? TERMINAL_CLASS_NAME : "overflow-hidden bg-terminal-app p-3"}`}
        role="group"
        aria-label={label}
      >
        <div ref={wrapRef} className="h-full overflow-hidden" />
      </div>
      {error ? <p role="alert" className="sr-only">{error}</p> : null}
    </div>
  );
}

export function SshTerminalTranscript({ host, transcript }: { host: string; transcript: string }) {
  const wrapRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const { terminal, dispose } = mountTerminal(wrap, true, true);
    terminal.write(transcript);
    return dispose;
  }, [transcript]);

  return (
    <div
      className={`mt-3 h-40 ${TERMINAL_CLASS_NAME}`}
      role="group"
      aria-label={m.settings_ssh_connection_terminal({ host: ltr(host) })}
    >
      <div ref={wrapRef} className="h-full overflow-hidden" />
    </div>
  );
}
