import { useEffect, useRef, useState } from "react";
import type { SlurmPreflight, SshPreflight } from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { Button } from "./ui";
import { mountTerminal } from "./terminal";

export type SshConnectResult =
  | { backend: "ssh"; result: SshPreflight }
  | { backend: "slurm"; result: SlurmPreflight };

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
  onComplete,
  onError,
  onCancel,
}: {
  host: string;
  backend: "ssh" | "slurm";
  onComplete: (result: SshConnectResult) => void;
  onError: (error: string) => void;
  onCancel: () => void;
}) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const socketRef = useRef<WebSocket | null>(null);
  const completeRef = useRef(onComplete);
  const errorRef = useRef(onError);
  const [attempt, setAttempt] = useState(0);
  const [error, setError] = useState<string | null>(null);
  completeRef.current = onComplete;
  errorRef.current = onError;

  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const { terminal, dispose } = mountTerminal(wrap, false);
    terminal.focus();
    const protocol = location.protocol === "https:" ? "wss:" : "ws:";
    const url = new URL("/api/settings/ssh/connect", `${protocol}//${location.host}`);
    url.searchParams.set("host", host);
    url.searchParams.set("backend", backend);
    const socket = new WebSocket(url);
    socket.binaryType = "arraybuffer";
    socketRef.current = socket;
    let completed = false;
    let failed = false;
    const fail = (message: string) => {
      if (failed) return;
      failed = true;
      setError(message);
      errorRef.current(message);
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
      const result = connectionResult(value);
      if (result) {
        completed = true;
        completeRef.current(result);
        socket.close();
        return;
      }
      const message = serverError(value);
      if (message) fail(message);
    };
    socket.onerror = () => fail(m.settings_ssh_connection_closed());
    socket.onclose = () => {
      if (!completed && !failed) fail(m.settings_ssh_connection_closed());
    };

    return () => {
      socket.onopen = null;
      socket.onmessage = null;
      socket.onerror = null;
      socket.onclose = null;
      input.dispose();
      resize.dispose();
      socket.close();
      socketRef.current = null;
      dispose();
    };
  }, [attempt, backend, host]);

  return (
    <div className="mt-3">
      <div
        ref={wrapRef}
        className="h-64 overflow-hidden rounded-md border border-border bg-background p-2"
        role="group"
        aria-label={m.settings_ssh_connection_terminal({ host: ltr(host) })}
      />
      {error ? <p role="alert" className="mt-2 whitespace-pre-wrap text-sm text-accent-red">{error}</p> : null}
      <div className="mt-3 flex justify-end gap-2">
        {error ? (
          <Button
            type="button"
            onClick={() => {
              setError(null);
              setAttempt((value) => value + 1);
            }}
          >
            {m.app_retry()}
          </Button>
        ) : null}
        <Button
          type="button"
          onClick={() => {
            socketRef.current?.close();
            onCancel();
          }}
        >
          {m.settings_page_cancel()}
        </Button>
      </div>
    </div>
  );
}
