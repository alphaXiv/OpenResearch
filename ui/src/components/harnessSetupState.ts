import type { Harness } from "../api";

export type SetupAction = "install" | "update" | "login";
export type SetupPhase = "preview" | "running" | "checking" | "success" | "error";

/** `unsupported` also covers states no update repairs; those set needsConfigRepair. */
export function setupAction(h: Pick<Harness, "installed" | "installBroken" | "authState" | "needsConfigRepair">): SetupAction {
  if (!h.installed || h.installBroken) return "install";
  return h.authState === "unsupported" && !h.needsConfigRepair ? "update" : "login";
}

/** No command fixes needsConfigRepair, so none may auto-start: open on the note. */
export function initialPhase(h: Pick<Harness, "needsConfigRepair">, action: SetupAction): SetupPhase {
  return h.needsConfigRepair ? "error" : action === "login" ? "running" : "preview";
}

/** Unmounting is the only gate on an unapproved start; `started` is the approval
 * and keeps a finished run's output. `preview` mounts to echo the command. */
export function terminalMounted(phase: SetupPhase, started: boolean): boolean {
  return started || phase === "preview";
}

/** Retry's action, re-derived after a re-check since `action` is fixed at mount.
 * Null keeps the mount-time choice. */
export function recheckedAction(current: Harness | undefined, action: SetupAction): SetupAction | null {
  if (!current || current.needsConfigRepair) return null;
  const next = setupAction(current);
  return next === action ? null : next;
}
