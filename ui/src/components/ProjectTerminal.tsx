import { useEffect, useRef, useState } from "react";
import { m } from "../paraglide/messages.js";
import { mountTerminal } from "./terminal";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

/** An interactive shell in the project's checkout (the session worktree when one exists). */
export function ProjectTerminal({ projectId, sessionId, active }: {
  projectId: string;
  sessionId: string | null;
  active: boolean;
}) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<ReturnType<typeof mountTerminal>["terminal"] | null>(null);
  const [ended, setEnded] = useState<string | null>(null);

  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const { terminal, dispose } = mountTerminal(wrap, false, true);
    terminalRef.current = terminal;
    const protocol = location.protocol === "https:" ? "wss:" : "ws:";
    const query = new URLSearchParams(sessionId ? { sessionId } : {});
    const url = new URL(`/api/projects/${encodeURIComponent(projectId)}/terminal?${query}`, `${protocol}//${location.host}`);
    const socket = new WebSocket(url);
    socket.binaryType = "arraybuffer";
    let finished = false;
    const finish = (message: string) => {
      if (finished) return;
      finished = true;
      terminal.options.disableStdin = true;
      terminal.writeln(`\r\n${message}`);
      setEnded(message);
    };

    const input = terminal.onData((data) => {
      if (socket.readyState === WebSocket.OPEN) socket.send(new TextEncoder().encode(data));
    });
    const resize = terminal.onResize(({ cols, rows }) => {
      if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: "resize", cols, rows }));
    });
    socket.onopen = () => {
      socket.send(JSON.stringify({ type: "resize", cols: terminal.cols, rows: terminal.rows }));
    };
    socket.onmessage = (event) => {
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
      if (value.type === "exit") finish(m.terminal_exited({ code: String(value.code) }));
      else if (value.type === "error" && typeof value.error === "string") finish(value.error);
    };
    socket.onclose = () => finish(m.terminal_disconnected());
    socket.onerror = () => finish(m.terminal_disconnected());

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
  }, [projectId, sessionId]);

  useEffect(() => {
    if (active && ended === null) terminalRef.current?.focus();
  }, [active, ended]);

  return (
    <div className="flex h-full min-h-0 flex-col bg-terminal p-2" role="group" aria-label={m.workspace_terminal()}>
      <div ref={wrapRef} className="h-full min-h-0 overflow-hidden" />
      {ended ? <p role="status" className="sr-only">{ended}</p> : null}
    </div>
  );
}
