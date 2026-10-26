import type { Harness } from "./api";

export function resolveHarnessDefaultModel(harness: Harness | undefined): string | null {
  if (!harness || harness.catalogPending || harness.models.length === 0) return null;
  if (harness.id === "opencode") return harness.models[0].id;
  if (harness.id === "claude-code") {
    if (!harness.agentReady) return null;
    return groupModels(harness.id, harness.models).current.find((model) => /\bopus\b/i.test(`${model.displayName ?? ""} ${model.id}`))?.id ?? null;
  }
  if (harness.id !== "codex") return null;
  if (!harness.agentReady) return null;

  let best = harness.models[0].id;
  let bestVersion: bigint[] | null = null;
  for (const model of harness.models) {
    const match = [model.id, model.displayName ?? ""]
      .map((name) => /gpt[\s._-]*(\d+(?:[._-]\d+)*)[\s._-]*sol\b/i.exec(name))
      .find(Boolean);
    if (!match) continue;
    const version = match[1].split(/[._-]/).map((part) => BigInt(part));
    let comparison = 0;
    if (bestVersion) {
      for (let i = 0; i < Math.max(version.length, bestVersion.length); i++) {
        if ((version[i] ?? 0n) === (bestVersion[i] ?? 0n)) continue;
        comparison = (version[i] ?? 0n) > (bestVersion[i] ?? 0n) ? 1 : -1;
        break;
      }
    }
    if (!bestVersion || comparison > 0) {
      best = model.id;
      bestVersion = version;
    }
  }
  return best;
}

export function formatCodexModel(name: string): string {
  return name.replace(/\bgpt[\s_-]*(\d+(?:\.\d+)*)(?:[\s_-]+)(astra|sol|terra|luna)\b/i, (_, version, family) => `GPT-${version} ${family[0].toUpperCase()}${family.slice(1).toLowerCase()}`);
}

export function groupModels<T extends { id: string; displayName?: string }>(harness: string, models: T[]): { current: T[]; legacy: T[] } {
  const families = harness === "claude-code" ? ["mythos", "fable", "opus", "sonnet", "haiku"] : harness === "codex" ? ["astra", "sol", "terra", "luna"] : [];
  if (!families.length) return { current: models, legacy: [] };
  const classified = models.map((model, index) => {
    const text = `${model.displayName ?? ""} ${model.id}`.toLowerCase();
    const family = families.find((name) => new RegExp(`\\b${name}\\b`).test(text));
    const version = family && (harness === "codex"
      ? /gpt[\s_-]*(\d+(?:[._-]\d+)*)/i.exec(text)?.[1]
      : new RegExp(`${family}[\\s_-]+(\\d+(?:[._-]\\d+)*)`, "i").exec(text)?.[1]);
    return { model, index, family, version: version?.split(/[._-]/).map(Number) };
  });
  const compare = (a: number[], b: number[]) => {
    for (let i = 0; i < Math.max(a.length, b.length); i++) {
      const difference = (b[i] ?? 0) - (a[i] ?? 0);
      if (difference) return difference;
    }
    return 0;
  };
  classified.sort((a, b) => {
    const aRank = a.family ? families.indexOf(a.family) : families.length;
    const bRank = b.family ? families.indexOf(b.family) : families.length;
    return aRank - bRank || (a.version && b.version ? compare(a.version, b.version) : 0) || a.index - b.index;
  });
  const seen = new Set<string>();
  const current: T[] = [];
  const legacy: T[] = [];
  for (const entry of classified) {
    if (entry.family && entry.version && seen.has(entry.family)) legacy.push(entry.model);
    else {
      current.push(entry.model);
      if (entry.family && entry.version) seen.add(entry.family);
    }
  }
  return { current, legacy };
}
