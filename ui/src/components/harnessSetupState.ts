import type { Harness } from "../api";

/** `upgrade` is OpenCode's: the user's own 1.x moves to 2, or OpenResearch's copy stands in. */
export type SetupAction = "install" | "update" | "login" | "upgrade";
export type SetupPhase = "preview" | "running" | "checking" | "error";

/** `unsupported` also covers states no update repairs; those set needsConfigRepair. */
export function setupAction(h: Pick<Harness, "installed" | "installBroken" | "authState" | "needsConfigRepair" | "adapterMissing">): SetupAction {
  if (h.adapterMissing || !h.installed || h.installBroken) return "install";
  return h.authState === "unsupported" && !h.needsConfigRepair ? "update" : "login";
}

/** No command fixes needsConfigRepair, so none may auto-start: open on the note. A sign-in,
 * an upgrade the user asked for, and a copy OpenResearch downloads itself start at once;
 * anything else changes the user's own system, so it asks first. */
export function initialPhase(h: Pick<Harness, "needsConfigRepair">, action: SetupAction, managed = false): SetupPhase {
  if (h.needsConfigRepair) return "error";
  return action === "login" || action === "upgrade" || (action === "install" && managed) ? "running" : "preview";
}

/** Retry's action, re-derived after a re-check since `action` is fixed at mount.
 * Null keeps the mount-time choice; an upgrade is only ever asked for. */
export function recheckedAction(current: Harness | undefined, action: SetupAction): SetupAction | null {
  if (!current || current.needsConfigRepair || action === "upgrade") return null;
  const next = setupAction(current);
  return next === action ? null : next;
}
