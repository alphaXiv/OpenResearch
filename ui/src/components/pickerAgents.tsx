import { useCallback } from "react";
import { useQuery } from "@tanstack/react-query";
import type { AgentIcon, AgentSelection, FoundProvider, Harness, LocalModelConnection, OpenCodeProvider } from "../api";
import { m } from "../paraglide/messages.js";
import { groupModelLines, type ModelOffer } from "../modelLines";
import { harnessDisplayName, harnessKey, selectionHarness } from "../modelSelection";
import { useWorkspace } from "../workspace";
import { BRAND_ICON, type Logo } from "./AgentIcon";
import { profileTile, providerRank, providerTile, sortHarnesses, TileMark } from "./AddAgentDialog";
import { HarnessLogo, logoMark } from "./HarnessLogo";
import { EndpointMark, endpointServer } from "./LocalModelSetup";
import { foundProviderLook, hostLogo } from "./providerIdentity";
import { FREE, lineName, offerOn, SectionMark } from "./ModelList";

/** A row of Settings → Agents, as the model picker offers it: one place a model can run. */
export interface PickerAgent {
  key: string;
  name: string;
  icon: AgentIcon;
  brand: Logo;
  harness: Harness;
  /** The OpenCode provider whose models this agent runs, the prefix of their ids. */
  provider?: string;
}

const FREE_PROVIDER = "opencode";
/** The prefix of the providers openresearch.sh's model feed hosts, whose models are free like OpenCode's own. */
const HOSTED_PREFIX = "openresearch-";

/** The OpenCode provider whose agent runs `model`: Free Models for the feed's hosted ones. */
const modelProvider = (model: string) => {
  const provider = model.split("/")[0];
  return provider.startsWith(HOSTED_PREFIX) ? FREE_PROVIDER : provider;
};

/** The icon a row of Settings → Agents shows, and the logo its brand icon draws. */
interface AgentMark {
  icon: AgentIcon;
  brand: Logo;
}

export function harnessMark(h: Harness): AgentMark {
  if (h.profile) {
    const tile = profileTile(h.id);
    return { icon: h.profile.icon ?? BRAND_ICON, brand: logoMark(h.profile.logo) ?? ((size) => <TileMark tile={tile} size={size} />) };
  }
  return { icon: h.label?.icon ?? BRAND_ICON, brand: logoMark(h.label?.logo) ?? ((size) => <HarnessLogo harness={h.id} logo={h.logo} size={size} />) };
}

export function providerMark(provider: OpenCodeProvider): AgentMark {
  const { tile } = providerTile(provider);
  return { icon: provider.icon ?? BRAND_ICON, brand: logoMark(provider.logo) ?? ((size) => <TileMark tile={tile} size={size} />) };
}

export function endpointMark(connection: LocalModelConnection): AgentMark {
  const server = endpointServer(connection);
  const remote = server?.icon ? undefined : logoMark(hostLogo(connection.baseUrl));
  return { icon: connection.icon ?? BRAND_ICON, brand: logoMark(connection.logo) ?? remote ?? ((size) => <EndpointMark server={server} size={size} />) };
}

export const FREE_MODELS_MARK: AgentMark = { icon: BRAND_ICON, brand: (size) => <SectionMark id={FREE} size={size} /> };

/** A provider set up in the user's own OpenCode, by its name and logo; a tile OpenResearch offers names the ones it knows. */
export function foundProviderMark(provider: FoundProvider): AgentMark & { name: string } {
  const { tile } = providerTile({ id: provider.id, name: provider.id });
  const known = "letter" in tile.mark ? undefined : { name: tile.name, logo: (size: number) => <TileMark tile={tile} size={size} /> };
  return foundProviderLook(provider, known);
}

/** An agent or account listed while signed out, with a way to sign in: an account OpenResearch keeps, or an agent a chat on this machine ran on. */
export function listedSignedOut(h: Harness) {
  return h.id !== "opencode" && h.installed && !h.installBroken && h.authState === "needsLogin" && Boolean(h.profile || h.hasChats);
}

/**
 * The agents that can run a model now, in Settings' order, less those turned off but the one `kept` picks. OpenCode
 * stands behind its providers.
 */
export function pickerAgents(harnesses: Harness[], providers: OpenCodeProvider[], endpoints: LocalModelConnection[], kept?: Picked | null): PickerAgent[] {
  const keptProvider = kept?.harness === "opencode" ? kept.model && modelProvider(kept.model) : undefined;
  const off = (h: Harness) => h.off && !(kept && h.id === kept.harness && (h.profile?.id ?? null) === (kept.profile ?? null));
  return sortHarnesses(harnesses).filter((h) => h.agentReady && (h.id === "opencode" || !off(h))).flatMap((h) => {
    if (h.id !== "opencode") return [{ key: harnessKey(h), name: h.profile?.name ?? harnessDisplayName(h), ...harnessMark(h), harness: h }];
    const used = new Set(h.models.map((model) => modelProvider(model.id)));
    const named = new Set([...providers.map((provider) => provider.id), ...endpoints.map((connection) => connection.id), FREE_PROVIDER]);
    // A server from before found providers names them only in its model ids.
    const found = h.foundProviders ?? [...used].filter((provider) => !named.has(provider)).map((id): FoundProvider => ({ id, sources: ["catalog"], removable: false }));
    // A server from before the per-provider switches turns everything it found off at once.
    const turnedOff = new Set(h.offProviders ?? (h.off ? found.map((provider) => provider.id) : []));
    const shown = (provider: string) => used.has(provider) && (!turnedOff.has(provider) || provider === keptProvider);
    const records = [...providers].sort((a, b) => providerRank(a) - providerRank(b)).filter((provider) => shown(provider.id));
    const connections = endpoints.filter((connection) => shown(connection.id));
    return [
      ...records.map((provider) => ({ key: `opencode:${provider.id}`, name: provider.name, ...providerMark(provider), harness: h, provider: provider.id })),
      ...connections.map((connection) => ({ key: `opencode:${connection.id}`, name: connection.name, ...endpointMark(connection), harness: h, provider: connection.id })),
      ...(used.has(FREE_PROVIDER) ? [{ key: `opencode:${FREE_PROVIDER}`, name: m.settings_free_models(), ...FREE_MODELS_MARK, harness: h, provider: FREE_PROVIDER }] : []),
      ...found.filter((provider) => shown(provider.id)).map((provider) => ({ key: `opencode:${provider.id}`, ...foundProviderMark(provider), harness: h, provider: provider.id })),
    ];
  });
}

/** Which of `agents` runs `model` on harness entry `h`. */
export function agentFor(agents: PickerAgent[], h: Harness, model: string | null): PickerAgent | undefined {
  if (h.id !== "opencode") return agents.find((agent) => agent.key === harnessKey(h));
  const provider = model && modelProvider(model);
  return agents.find((agent) => agent.harness.id === "opencode" && agent.provider === provider);
}

/** An agent's models as offers, less the moving aliases its versions stand for, unless `kept`. OpenCode's providers
 * each offer their own. */
export function agentOffers(agent: PickerAgent, kept?: string | null): ModelOffer<PickerAgent>[] {
  const h = agent.harness;
  const models = agent.provider ? h.models.filter((model) => modelProvider(model.id) === agent.provider) : h.models;
  return models.filter((model) => !model.parsed.superseded || model.id === kept).map((model) => ({ tile: agent, model }));
}

interface PickerCatalog {
  catalog: Harness[];
  providers: OpenCodeProvider[];
  endpoints: LocalModelConnection[];
  hiddenModels: readonly string[];
}

type Picked = Pick<AgentSelection, "harness" | "profile" | "model">;

/** What the model picker lists while `value` is picked, which keeps its agent and model listed even when turned off. */
export function pickerLines({ catalog, providers, endpoints, hiddenModels }: PickerCatalog, value: Picked | null | undefined, sessionHarness?: Harness) {
  // An agent's own sign-in steps aside while it's signed out and an account OpenResearch keeps stands in;
  // accounts list beside their agent.
  const position = (h: Harness) => catalog.findIndex((other) => other.id === h.id);
  const offered = catalog
    .filter((h) => h.profile || h.agentReady || !catalog.some((other) => other.profile && other.id === h.id))
    .sort((a, b) => position(a) - position(b));
  const harnesses = sessionHarness ? [sessionHarness, ...offered.filter((h) => h.id !== sessionHarness.id)] : offered;

  const agents = pickerAgents(harnesses, providers, endpoints, value);
  const selectedHarness = selectionHarness(harnesses, value);
  const selectedModel = value?.model ?? null;
  const selectedAgent = selectedHarness ? agentFor(agents, selectedHarness, selectedModel) : undefined;
  const offers = agents.flatMap((agent) => agentOffers(agent, agent === selectedAgent ? selectedModel : null));

  const hidden = new Set(hiddenModels);
  const lines = groupModelLines(offers.filter((offer) => !hidden.has(offer.model.parsed.key) || (offer.tile === selectedAgent && offer.model.id === selectedModel)));
  const selectedLine = lines.find((line) => offerOn(line, selectedAgent, selectedModel));
  return { harnesses, agents, offers, selectedHarness, selectedModel, selectedAgent, lines, selectedLine };
}

const NONE: never[] = [];

/** Each workspace's catalog, its lines listed with nothing picked, and the names found so far. */
const pickedNames = new WeakMap<Harness[], { catalog: PickerCatalog; listed: ReturnType<typeof pickerLines>; names: Map<string, string | null> }>();

/** A model's name as the picker's trigger shows it, or null when its agent doesn't list it. */
function pickerModelName(catalog: PickerCatalog, value: Picked) {
  const known = pickedNames.get(catalog.catalog);
  const fresh = known && (Object.keys(catalog) as (keyof PickerCatalog)[]).every((field) => known.catalog[field] === catalog[field]);
  const cached = fresh ? known : { catalog, listed: pickerLines(catalog, null), names: new Map<string, string | null>() };
  pickedNames.set(catalog.catalog, cached);
  const key = `${value.harness}\u0000${value.profile ?? ""}\u0000${value.model ?? ""}`;
  if (cached.names.has(key)) return cached.names.get(key)!;

  // Most picks change nothing the picker lists, so the lines listed with nothing picked name them.
  const { listed } = cached;
  const h = selectionHarness(listed.harnesses, value);
  const agent = h && !h.off && agentFor(listed.agents, h, value.model);
  const line = (agent && listed.lines.find((line) => offerOn(line, agent, value.model))) || pickerLines(catalog, value).selectedLine;
  const name = line ? lineName(line) : null;
  cached.names.set(key, name);
  return name;
}

/** Names models as the picker does, for chats and notices outside it. */
export function useModelName(): (value: Picked) => string | null {
  const { getHarnessesQuery, getOpenCodeProvidersQuery, getLocalModelsQuery, getUiStateQuery } = useWorkspace().queries;
  const { data: catalog } = useQuery(getHarnessesQuery());
  const { data: providers = NONE } = useQuery(getOpenCodeProvidersQuery());
  const { data: endpoints = NONE } = useQuery(getLocalModelsQuery());
  const { data: uiState } = useQuery(getUiStateQuery());
  const hiddenModels = uiState?.hiddenModels ?? NONE;
  return useCallback(
    (value: Picked) => (catalog ? pickerModelName({ catalog, providers, endpoints, hiddenModels }, value) : null),
    [catalog, providers, endpoints, hiddenModels],
  );
}
