import type { HarnessModel, ModelSightings, ModelVariant } from "./api";

/** Effort ids in rising order, as Cursor and Antigravity spell them into model ids. */
export const VARIANT_EFFORTS = ["none", "minimal", "low", "medium", "high", "xhigh", "max"] as const;

/** Claude's tiers, biggest first. */
export const CLAUDE_TIERS = ["mythos", "fable", "opus", "sonnet", "haiku"];

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
    const { key, family, series, version, label } = offer.model.parsed;
    const line = lines.get(key) ?? { key, family, series, version, label, offers: [], older: false };
    line.offers.push(offer);
    lines.set(key, line);
    if (!familyOrder.has(family)) familyOrder.set(family, familyOrder.size);
    const best = newest.get(series);
    if (version && (!best || compareVersions(version, best) > 0)) newest.set(series, version);
  }
  const spelled = (label: string) => label.split(/[\s_-]+/).length;
  return [...lines.values()]
    .map((line) => {
      const best = newest.get(line.series);
      const label = line.offers.map((offer) => offer.model.parsed.label).reduce((a, b) => (spelled(b) > spelled(a) ? b : a));
      return { ...line, label, older: Boolean(line.version && best && compareVersions(line.version, best) < 0) };
    })
    .sort((a, b) => familyOrder.get(a.family)! - familyOrder.get(b.family)! || compareVersions(b.version ?? [Infinity], a.version ?? [Infinity]));
}

/** The offer a tile runs by default. */
export function defaultOffer<Tile>(offers: ModelOffer<Tile>[]): ModelOffer<Tile> | undefined {
  const rank = (offer: ModelOffer<Tile>) => offer.model.parsed.variantRank;
  return offers.reduce<ModelOffer<Tile> | undefined>((best, offer) => (!best || rank(offer) < rank(best) ? offer : best), undefined);
}

/** The sibling with `change` applied, keeping as much of `from` as the catalog allows. */
export function variantOffer<Tile>(offers: ModelOffer<Tile>[], from: ModelVariant, change: Partial<ModelVariant>): ModelOffer<Tile> | undefined {
  const want = { ...from, ...change };
  const fits = offers.filter((offer) => (Object.keys(change) as (keyof ModelVariant)[]).every((field) => offer.model.parsed.variant[field] === want[field]));
  const kept = (offer: ModelOffer<Tile>) => (Object.keys(want) as (keyof ModelVariant)[]).filter((field) => offer.model.parsed.variant[field] === want[field]).length;
  return fits.reduce<ModelOffer<Tile> | undefined>((best, offer) => (!best || kept(offer) > kept(best) ? offer : best), undefined);
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
