import { useEffect, useRef } from "react";
import { fetchLog } from "../api";
import { onRunLog } from "../events";
import { mountTerminal } from "./terminal";

function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/**
 * Live log terminal for one run. Backfills from /api/runs/{id}/log, then
 * follows `run.log` SSE deltas. Fast path writes an in-order delta directly;
 * any gap (missed event, reconnect) falls back to a serialized fetch-sync
 * from the current byte offset, so output is never duplicated or reordered.
 */
export function LogTerminal({ runId }: { runId: string }) {
  const wrapRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const { terminal: term, dispose } = mountTerminal(wrap, true);

    let disposed = false;
    let nextOffset = 0;
    let syncing = false;
    let syncAgain = false;

    async function sync() {
      if (syncing) {
        syncAgain = true;
        return;
      }
      syncing = true;
      try {
        for (;;) {
          const chunk = await fetchLog(runId, nextOffset);
          if (disposed) return;
          if (chunk.dataBase64) term.write(b64ToBytes(chunk.dataBase64));
          nextOffset = chunk.nextOffset;
          if (chunk.eof) break;
        }
      } catch {
        // transient; the next run.log event retries
      } finally {
        syncing = false;
        if (syncAgain && !disposed) {
          syncAgain = false;
          void sync();
        }
      }
    }

    const unsubscribe = onRunLog(runId, (ev) => {
      if (disposed) return;
      const bytes = b64ToBytes(ev.dataBase64);
      if (!syncing && ev.offset === nextOffset) {
        term.write(bytes);
        nextOffset += bytes.length;
      } else if (ev.offset + bytes.length > nextOffset) {
        void sync();
      }
    });
    void sync();

    return () => {
      disposed = true;
      unsubscribe();
      dispose();
    };
  }, [runId]);

  return <div ref={wrapRef} className="h-full w-full" />;
}
