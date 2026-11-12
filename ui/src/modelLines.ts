import type { HarnessModel, ModelSightings } from "./api";

/** Effort ids in rising order, as Cursor and Antigravity spell them into model ids. */
export const VARIANT_EFFORTS = ["none", "minimal", "low", "medium", "high", "xhigh", "max"] as const;

/** What an offer adds to its model: Cursor's and Antigravity's effort, speed and thinking, Claude Code's long context. */
export interface ModelVariant {
  effort: string | null;
  fast: boolean;
  thinking: boolean;
  context: string | null;
}

interface ParsedModel {
  /** The same for one model in every agent's catalog. */
  key: string;
  /** `key` without its version or free tier: what a favorite stars. */
  family: string;
  /** `family` without tier words a lab reuses across versions: a newer version of any tier makes an older one old. */
  series: string;
  version: number[] | null;
  /** The catalog's name without variant words, dates or asides. */
  label: string;
  variant: ModelVariant;
  /** How many variant words the catalog's name spells out; a tile's default variant spells the fewest. */
  variantWords: number;
}

/** Claude's tiers, biggest first. */
export const CLAUDE_TIERS = ["mythos", "fable", "opus", "sonnet", "haiku"];
const VARIANT_WORDS = new Set([...VARIANT_EFFORTS, "extra", "fast", "thinking", "no"]);
const TIER_WORDS = new Set(["flash", "pro", "max", "plus", "mini", "nano", "lite", "air", "turbo", "ultra", "fast", "instant", "preview", "exp", "chat", "instruct", "thinking", "reasoning", "reasoner"]);
const isSize = (token: string) => /^[ae]?\d+(\.\d+)?b$/.test(token);
/** A name's words, a version split from the word it's glued to ("Qwen3.8", "K3") but a size kept whole ("A3B"). */
const nameWords = (text: string) => text.toLowerCase().replace(/([a-z])(\d)(?!\d*(\.\d+)?b\b)/g, "$1 $2").split(/[\s_-]+/).filter(Boolean);
const isVersion = (token: string) => /^\d+(\.\d+)*$/.test(token);
const isDate = (token: string) => /^(\d{4}|\d{8})$/.test(token);
const isContext = (token: string) => /^\d+[mk]$/i.test(token);

/** Cursor and Antigravity list each effort, speed and thinking mode as a model of its own. */
function spellsVariants(harness: string) {
  return harness === "cursor" || harness === "antigravity";
}

function idVariant(id: string, harness: string): ModelVariant {
  const variant: ModelVariant = { effort: null, fast: false, thinking: false, context: null };
  if (harness === "claude-code") {
    variant.context = /\[(\d+[mk])\]$/i.exec(id)?.[1].toLowerCase() ?? null;
    return variant;
  }
  if (!spellsVariants(harness)) return variant;
  const parts = id.toLowerCase().split("-");
  while (parts.length > 1) {
    const last = parts[parts.length - 1];
    if (last === "fast") variant.fast = true;
    else if (last === "thinking") variant.thinking = true;
    else if ((VARIANT_EFFORTS as readonly string[]).includes(last) && variant.effort === null) {
      variant.effort = last === "high" && parts[parts.length - 2] === "extra" ? (parts.pop(), "xhigh") : last;
    } else break;
    parts.pop();
  }
  return variant;
}

/** One catalog entry read as a model: its key across catalogs, its version, and its variant. A free tier's name drops "Free" for a badge. */
export function parseModel(harness: string, model: Pick<HarnessModel, "id" | "displayName" | "free">): ParsedModel {
  const variant = idVariant(model.id, harness);
  const asides = [...(model.displayName ?? "").matchAll(/\(([^)]*)\)/g)].map((match) => match[1].toLowerCase());
  if (asides.includes("thinking")) variant.thinking = true;
  const name = (model.displayName || model.id.replace(/^.*\//, ""))
    .replace(/[\u200B-\u200D\uFEFF]/g, "")
    .replace(/\([^)]*\)/g, " ")
    .replace(/\bwith \d+[mk] context\b/i, " ")
    .replace(/\[\d+[mk]\]$/i, "")
    .replace(/[\s:_-]+free\s*$/i, (word) => (model.free ? "" : word));
  let variantWords = 0;
  let seenVersion = false;
  const kept: string[] = [];
  const keys: string[] = [];
  for (const word of name.split(/\s+/).filter(Boolean)) {
    const lower = word.toLowerCase();
    const parts = nameWords(lower);
    if (spellsVariants(harness) && (VARIANT_WORDS.has(lower) || isContext(lower))) {
      if (isContext(lower)) variant.context = lower;
      variantWords++;
      continue;
    }
    if (seenVersion && isDate(lower)) continue;
    if (parts.some(isVersion)) seenVersion = true;
    kept.push(word);
    keys.push(...parts);
  }
  if (harness === "claude-code" && variant.context) variantWords++;
  const versionIndex = keys.findIndex(isVersion);
  const version = versionIndex < 0 ? null : keys[versionIndex].split(".").map(Number);
  const familyWords = keys.filter((_, index) => index !== versionIndex);
  if (familyWords.some((word) => CLAUDE_TIERS.includes(word)) && !familyWords.includes("claude")) familyWords.push("claude");
  const family = [...new Set(familyWords)].sort().join(" ");
  const series = [...new Set(familyWords.filter((word) => !TIER_WORDS.has(word) && !isSize(word)))].sort().join(" ") || family;
  return {
    key: `${version ? `${family}@${version.join(".")}` : family}${model.free ? ":free" : ""}`,
    family,
    series,
    version,
    label: kept.join(" "),
    variant,
    variantWords,
  };
}

function compareVersions(a: number[], b: number[]) {
  for (let index = 0; index < Math.max(a.length, b.length); index++) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference) return difference;
  }
  return 0;
}

export interface ModelOffer<Tile> {
  tile: Tile;
  model: HarnessModel;
  parsed: ParsedModel;
}

/** One model across every agent that offers it. */
export interface ModelLine<Tile> {
  key: string;
  family: string;
  series: string;
  version: number[] | null;
  /** The catalog name that spells the most of the model, so "Claude Opus 5.5" beats "Opus 5.5". */
  label: string;
  offers: ModelOffer<Tile>[];
  /** A newer version of its series is offered too. */
  older: boolean;
}

/** Offers grouped into models: families in the order they first appear, each newest first. */
export function groupModelLines<Tile>(offers: ModelOffer<Tile>[]): ModelLine<Tile>[] {
  const lines = new Map<string, ModelLine<Tile>>();
  const familyOrder = new Map<string, number>();
  const newest = new Map<string, number[]>();
  for (const offer of offers) {
    const { key, family, series, version } = offer.parsed;
    const line = lines.get(key) ?? { key, family, series, version, label: offer.parsed.label, offers: [], older: false };
    line.offers.push(offer);
    lines.set(key, line);
    if (!familyOrder.has(family)) familyOrder.set(family, familyOrder.size);
    const best = newest.get(series);
    if (version && (!best || compareVersions(version, best) > 0)) newest.set(series, version);
  }
  const spelled = (label: string, line: ModelLine<Tile>) => {
    const words = new Set(nameWords(label));
    return [...line.family.split(" "), ...(line.version ? [line.version.join(".")] : [])].filter((word) => words.has(word)).length;
  };
  return [...lines.values()]
    .map((line) => {
      const best = newest.get(line.series);
      const label = line.offers.map((offer) => offer.parsed.label).reduce((a, b) => (spelled(b, line) > spelled(a, line) ? b : a));
      return { ...line, label, older: Boolean(line.version && best && compareVersions(line.version, best) < 0) };
    })
    .sort((a, b) => familyOrder.get(a.family)! - familyOrder.get(b.family)! || compareVersions(b.version ?? [Infinity], a.version ?? [Infinity]));
}

/** The offer a tile runs by default: the one whose name spells the least, medium effort breaking a tie. */
export function defaultOffer<Tile>(offers: ModelOffer<Tile>[]): ModelOffer<Tile> | undefined {
  const rank = (offer: ModelOffer<Tile>) => offer.parsed.variantWords * 2 + (offer.parsed.variant.effort === "medium" ? 0 : 1);
  return offers.reduce<ModelOffer<Tile> | undefined>((best, offer) => (!best || rank(offer) < rank(best) ? offer : best), undefined);
}

/** The sibling with `change` applied, keeping as much of `from` as the catalog allows. */
export function variantOffer<Tile>(offers: ModelOffer<Tile>[], from: ModelVariant, change: Partial<ModelVariant>): ModelOffer<Tile> | undefined {
  const want = { ...from, ...change };
  const fits = offers.filter((offer) => (Object.keys(change) as (keyof ModelVariant)[]).every((field) => offer.parsed.variant[field] === want[field]));
  const kept = (offer: ModelOffer<Tile>) => (Object.keys(want) as (keyof ModelVariant)[]).filter((field) => offer.parsed.variant[field] === want[field]).length;
  return fits.reduce<ModelOffer<Tile> | undefined>((best, offer) => (!best || kept(offer) > kept(best) ? offer : best), undefined);
}

/** How Settings turns a model off: its delegate id, its free tier apart. Models the server doesn't name can't be turned off. */
export function hideId(model: Pick<HarnessModel, "canonical" | "free">) {
  return model.canonical && `${model.canonical}${model.free ? ":free" : ""}`;
}

/** How long a model an agent newly offers stays new. */
export const NEW_FOR = 7 * 24 * 60 * 60 * 1000;

/**
 * `sightings` with `lines` seen at `now`, or `sightings` itself when nothing changed. A model is new when an agent
 * already seen offers it, unless it's an older version; the first sighting and a newly added agent bring nothing new.
 */
export function sight<Tile extends { key: string }>(sightings: ModelSightings | null, lines: ModelLine<Tile>[], now: number): ModelSightings {
  const known = new Set(sightings?.agents);
  const agents = new Set(known);
  const models = { ...sightings?.models };
  let changed = !sightings;
  for (const line of lines) {
    for (const offer of line.offers) agents.add(offer.tile.key);
    if (line.key in models) continue;
    models[line.key] = sightings && !line.older && line.offers.some((offer) => known.has(offer.tile.key)) ? now : 0;
    changed = true;
  }
  if (!changed && agents.size === known.size) return sightings!;
  return { agents: [...agents], models, openedAt: sightings?.openedAt ?? now };
}
