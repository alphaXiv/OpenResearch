import type { Harness } from "../api";

export type SetupAction = "install" | "update" | "login";
export type SetupPhase = "preview" | "running" | "checking" | "error";

/** `unsupported` also covers states no update repairs; those set needsConfigRepair. */
export function setupAction(h: Pick<Harness, "installed" | "installBroken" | "authState" | "needsConfigRepair" | "adapterMissing">): SetupAction {
  if (h.adapterMissing || !h.installed || h.installBroken) return "install";
  return h.authState === "unsupported" && !h.needsConfigRepair ? "update" : "login";
}

/** No command fixes needsConfigRepair, so none may auto-start: open on the note. A sign-in,
 * and a copy OpenResearch downloads itself start at once;
 * anything else changes the user's own system, so it asks first. */
export function initialPhase(h: Pick<Harness, "needsConfigRepair">, action: SetupAction, managed = false): SetupPhase {
  if (h.needsConfigRepair) return "error";
  return action === "login" || (action === "install" && managed) ? "running" : "preview";
}

/** Retry's action, re-derived after a re-check since `action` is fixed at mount.
 * Null keeps the mount-time choice. */
export function recheckedAction(current: Harness | undefined, action: SetupAction): SetupAction | null {
  if (!current || current.needsConfigRepair) return null;
  const next = setupAction(current);
  return next === action ? null : next;
}
