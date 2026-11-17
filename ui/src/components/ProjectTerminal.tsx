import { useTerminalSession } from "./useTerminalSession";
import { useWorkspace } from "../workspace";
import { useEffect, useRef, useState } from "react";
import { m } from "../paraglide/messages.js";
import { Button } from "./ui";
import { isRecord } from "../workspaceState";
import { mountTerminal } from "./terminal";

/** A lost connection says so only once reconnecting takes longer than a quick restart. */
const RECONNECTING_AFTER_MS = 1000;
const RECONNECT_DELAYS_MS = [250, 500, 1000, 2000, 5000];

type Ended = { message: string; action: "restart" | "show" };

/** An interactive shell in the project's checkout (the session worktree when one exists). It
 * lives on the server under `terminalId`, so a reload or a server restart returns to it. */
export function ProjectTerminal({ projectId, sessionId, terminalId, active, onRestart }: {
  projectId: string;
  sessionId: string | null;
  terminalId: string;
  active: boolean;
  /** Replaces this tab's shell with a new one. */
  onRestart: () => void;
}) {
  const { socketUrl } = useWorkspace();
  const wrapRef = useRef<HTMLDivElement>(null);
  const [ended, setEnded] = useState<Ended | null>(null);
  const [reconnecting, setReconnecting] = useState(false);
  const [generation, setGeneration] = useState(0);

  const session = useTerminalSession(wrapRef, JSON.stringify([projectId, sessionId, terminalId, generation]), (wrap) => {
    const mounted = mountTerminal(wrap, false, true, "app");
    const { terminal, dispose } = mounted;
    setEnded(null);
    setReconnecting(false);
    const url = socketUrl(`/api/projects/${encodeURIComponent(projectId)}/terminal`);
    if (sessionId) url.searchParams.set("sessionId", sessionId);
    url.searchParams.set("terminalId", terminalId);
    let socket: WebSocket | null = null;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let quiet: ReturnType<typeof setTimeout> | undefined;
    let failures = 0;
    let finished = false;

    const finish = (next: Ended) => {
      if (finished) return;
      finished = true;
      clearTimeout(quiet);
      setReconnecting(false);
      terminal.options.disableStdin = true;
      terminal.blur();
      terminal.writeln(`\r\n${next.message}`);
      setEnded(next);
    };
    const lost = () => {
      if (finished) return;
      socket = null;
      quiet ??= setTimeout(() => setReconnecting(true), RECONNECTING_AFTER_MS);
      retry = setTimeout(connect, RECONNECT_DELAYS_MS[Math.min(failures, RECONNECT_DELAYS_MS.length - 1)]);
      failures++;
    };
    const connect = () => {
      const current = new WebSocket(url);
      socket = current;
      current.binaryType = "arraybuffer";
      current.onopen = () => {
        failures = 0;
        clearTimeout(quiet);
        quiet = undefined;
        setReconnecting(false);
        // The server replays the shell's scrollback from the start of what it kept.
        terminal.reset();
        current.send(JSON.stringify({ type: "resize", cols: terminal.cols, rows: terminal.rows }));
      };
      current.onmessage = (event) => {
        if (event.data instanceof ArrayBuffer) {
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
        if (!isRecord(value)) return;
        if (value.type === "exit") finish({ message: m.workspace_terminal_exited({ code: String(value.code) }), action: "restart" });
        else if (value.type === "gone") finish({ message: m.workspace_terminal_gone(), action: "restart" });
        else if (value.type === "elsewhere") finish({ message: m.workspace_terminal_elsewhere(), action: "show" });
        else if (value.type === "error" && typeof value.error === "string") finish({ message: value.error, action: "restart" });
      };
      current.onclose = () => { if (socket === current) lost(); };
    };

    const input = terminal.onData((data) => {
      if (socket?.readyState === WebSocket.OPEN) socket.send(new TextEncoder().encode(data));
    });
    const resize = terminal.onResize(({ cols, rows }) => {
      if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: "resize", cols, rows }));
    });
    connect();

    return { ...mounted, dispose: () => {
      finished = true;
      clearTimeout(retry);
      clearTimeout(quiet);
      const current = socket;
      socket = null;
      if (current) {
        current.onopen = null;
        current.onmessage = null;
        current.onclose = null;
        current.close();
      }
      input.dispose();
      resize.dispose();
      dispose();
    } };
  });

  useEffect(() => {
    if (!active || ended !== null) return;
    // A terminal mounted while hidden measured a zero-size cell; refit once visible.
    const frame = requestAnimationFrame(() => {
      session.current?.mounted.fit();
      session.current?.mounted.terminal.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [active, ended, generation]);

  const status = ended?.message ?? (reconnecting ? m.workspace_terminal_reconnecting() : "");
  return (
    <div className="flex h-full min-h-0 flex-col bg-terminal-app p-2" role="group" aria-label={m.workspace_terminal()}>
      <div ref={wrapRef} className="min-h-0 flex-1 overflow-hidden" />
      {/* Mounted whatever the state: a live region inserted with its text is missed by screen readers. */}
      <p role="status" aria-live="polite" className="sr-only">{status}</p>
      {ended !== null ? (
        <div className="flex shrink-0 justify-end pt-2">
          {ended.action === "show"
            ? <Button autoFocus onClick={() => setGeneration((value) => value + 1)}>{m.workspace_terminal_show_here()}</Button>
            : <Button autoFocus onClick={onRestart}>{m.workspace_terminal_restart()}</Button>}
        </div>
      ) : reconnecting && (
        <p aria-hidden className="shrink-0 pt-2 text-xs text-subtext">{m.workspace_terminal_reconnecting()}</p>
      )}
    </div>
  );
}
