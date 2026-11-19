import type { RunMonitor } from "./types";

type Revision = Pick<RunMonitor, "rev">;

/** The published revision keeps increasing across log generations. */
export function monitorBehind(incoming: Revision | null | undefined, current: Revision): boolean {
  return incoming != null && incoming.rev > current.rev;
}
