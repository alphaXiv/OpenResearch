import type { EngineHost, HostStatus, Run } from "./api";

// The engine keeps one connection per (alias, root): a host can serve runs
// under several roots, and one root's auth failure must not hide behind another's.

export function upsertEngineHost(hosts: EngineHost[], host: EngineHost): EngineHost[] {
  return [...hosts.filter((item) => item.alias !== host.alias || item.root !== host.root), host];
}

const SEVERITY: Record<HostStatus, number> = { needs_auth: 4, backoff: 3, connecting: 2, live: 1, idle: 0 };

/** One state per alias for views that show a host once: the worst connection wins. */
export function worstPerAlias(hosts: readonly EngineHost[]): Map<string, EngineHost> {
  const byAlias = new Map<string, EngineHost>();
  for (const host of hosts) {
    const current = byAlias.get(host.alias);
    if (!current || SEVERITY[host.status] > SEVERITY[current.status]) byAlias.set(host.alias, host);
  }
  return byAlias;
}

/** The root a run's engine handle lives under, when its backend exposes one. */
function runRoot(run: Pick<Run, "backend">): string | null {
  const handle = run.backend?.handle;
  return typeof handle === "object" && handle !== null && "root" in handle && typeof handle.root === "string" ? handle.root : null;
}

/** The connection a run depends on: its own root when known, else the alias's worst. */
export function hostForRun(hosts: readonly EngineHost[] | undefined, run: Pick<Run, "host" | "backend">): EngineHost | undefined {
  if (!hosts || !run.host) return undefined;
  const root = runRoot(run);
  return hosts.find((host) => host.alias === run.host && host.root === root)
    ?? worstPerAlias(hosts.filter((host) => host.alias === run.host)).get(run.host);
}
