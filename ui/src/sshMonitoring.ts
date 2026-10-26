import type { Run } from "./api";

export interface SshObservation {
  mode: "auto" | "on-demand";
  lastObservedAt?: number | null;
  lastLogSyncAt?: number | null;
  lastError?: string | null;
  logsComplete?: boolean;
}

export function sshObservation(run: Pick<Run, "backend">): SshObservation | null {
  const backend = run.backend;
  if (backend?.kind !== "ssh_job" || backend.sshContainer || backend.rentalLease) return null;
  const value = backend.monitoring as Partial<SshObservation> | undefined;
  return { ...value, mode: value?.mode === "on-demand" ? "on-demand" : "auto" };
}
