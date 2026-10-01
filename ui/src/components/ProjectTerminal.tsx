import { useEffect, useRef, useState } from "react";
import { m } from "../paraglide/messages.js";
import { Button } from "./ui";
import { isRecord } from "../workspaceState";
import { mountTerminal } from "./terminal";

/** An interactive shell in the project's checkout (the session worktree when one exists). */
export function ProjectTerminal({ projectId, sessionId, active }: {
  projectId: string;
  sessionId: string | null;
  active: boolean;
}) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const mountRef = useRef<ReturnType<typeof mountTerminal> | null>(null);
  const [ended, setEnded] = useState<string | null>(null);
  const [generation, setGeneration] = useState(0);

  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const mounted = mountTerminal(wrap, false, true, "app");
    const { terminal, dispose } = mounted;
    mountRef.current = mounted;
    setEnded(null);
    const protocol = location.protocol === "https:" ? "wss:" : "ws:";
    const url = new URL(`/api/projects/${encodeURIComponent(projectId)}/terminal`, `${protocol}//${location.host}`);
    if (sessionId) url.searchParams.set("sessionId", sessionId);
    const socket = new WebSocket(url);
    socket.binaryType = "arraybuffer";
    let finished = false;
    const finish = (message: string) => {
      if (finished) return;
      finished = true;
      terminal.options.disableStdin = true;
      terminal.blur();
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
      if (value.type === "exit") finish(m.workspace_terminal_exited({ code: String(value.code) }));
      else if (value.type === "error" && typeof value.error === "string") finish(value.error);
    };
    socket.onclose = () => finish(m.workspace_terminal_disconnected());
    socket.onerror = () => finish(m.workspace_terminal_disconnected());

    return () => {
      socket.onopen = null;
      socket.onmessage = null;
      socket.onerror = null;
      socket.onclose = null;
      input.dispose();
      resize.dispose();
      socket.close();
      mountRef.current = null;
      dispose();
    };
  }, [projectId, sessionId, generation]);

  useEffect(() => {
    if (!active || ended !== null) return;
    // A terminal mounted while hidden measured a zero-size cell; refit once visible.
    const frame = requestAnimationFrame(() => {
      mountRef.current?.fit();
      mountRef.current?.terminal.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [active, ended, generation]);

  return (
    <div className="flex h-full min-h-0 flex-col bg-terminal-app p-2" role="group" aria-label={m.workspace_terminal()}>
      <div ref={wrapRef} className="min-h-0 flex-1 overflow-hidden" />
      {/* Mounted whatever the state: a live region inserted with its text is missed by screen readers. */}
      <p role="status" aria-live="polite" className="sr-only">{ended ?? ""}</p>
      {ended !== null && (
        <div className="flex shrink-0 justify-end pt-2">
          <Button autoFocus onClick={() => setGeneration((value) => value + 1)}>{m.workspace_terminal_restart()}</Button>
        </div>
      )}
    </div>
  );
}
