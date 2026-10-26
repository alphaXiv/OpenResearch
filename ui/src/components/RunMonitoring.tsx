import { useState } from "react";
import { refreshRun, type Run } from "../api";
import { m } from "../paraglide/messages.js";
import { sshObservation } from "../sshMonitoring";
import { Button } from "./ui";

/** Reads cached metadata. Only explicit actions contact the remote host. */
export function RunMonitoring({ run }: { run: Run }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshed, setRefreshed] = useState<Run | null>(null);
  const latest = refreshed?.id === run.id && refreshed.updatedAt > run.updatedAt ? refreshed : run;
  const observation = sshObservation(latest);
  if (!observation) return null;
  const terminal = ["done", "failed", "cancelled", "lost"].includes(latest.status);
  async function refresh(logs: boolean) {
    setBusy(true); setError(null);
    try { setRefreshed((await refreshRun(run.id, logs)).run); }
    catch (err) { setError(err instanceof Error ? err.message : String(err)); }
    finally { setBusy(false); }
  }
  return <div className="flex shrink-0 flex-col gap-1 border-b border-border px-3 py-2 text-xs text-subtext">
    <div className="flex flex-wrap items-center gap-2">
      <span>{observation.mode === "on-demand" ? m.ssh_monitor_on_demand() : m.ssh_monitor_auto()}</span>
      {observation.lastObservedAt
        ? <span>{m.ssh_monitor_observed({ time: new Date(observation.lastObservedAt).toLocaleString() })}</span>
        : observation.mode === "on-demand" && <span>{m.ssh_monitor_unobserved()}</span>}
      <Button size="small" disabled={busy || terminal} onClick={() => void refresh(false)}>{m.ssh_monitor_refresh()}</Button>
      <Button size="small" disabled={busy || (terminal && observation.logsComplete === true)} onClick={() => void refresh(true)}>{m.ssh_monitor_sync_logs()}</Button>
    </div>
    {observation.mode === "on-demand" && <span>{m.ssh_monitor_cached_hint()}</span>}
    {latest.cancelRequested && !terminal && <span>{m.ssh_monitor_cancel_pending()}</span>}
    {terminal && observation.logsComplete === false && <span>{m.ssh_monitor_logs_pending()}</span>}
    {(error || observation.lastError) && <span role="alert" className="text-accent-red">{error || observation.lastError}</span>}
  </div>;
}
