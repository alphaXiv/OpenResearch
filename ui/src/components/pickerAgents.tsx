import { useCallback } from "react";
import { useQuery } from "@tanstack/react-query";
import type { AgentIcon, AgentSelection, Harness, LocalModelConnection, OpenCodeProvider } from "../api";
import { m } from "../paraglide/messages.js";
import { groupModelLines, type ModelOffer } from "../modelLines";
import { harnessDisplayName, harnessKey, selectionHarness } from "../modelSelection";
import { useWorkspace } from "../workspace";
import { BRAND_ICON, type Logo } from "./AgentIcon";
import { profileTile, providerRank, providerTile, sortHarnesses, TileMark } from "./AddAgentDialog";
import { HarnessLogo } from "./HarnessLogo";
import { EndpointMark, endpointServer } from "./LocalModelSetup";
import { lineName, offerOn } from "./ModelList";

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

/** The icon a row of Settings → Agents shows, and the logo its brand icon draws. */
interface AgentMark {
  icon: AgentIcon;
  brand: Logo;
}

export function harnessMark(h: Harness): AgentMark {
  return h.profile
    ? { icon: h.profile.icon ?? BRAND_ICON, brand: (size) => <TileMark tile={profileTile(h.id)} size={size} /> }
    : { icon: h.label?.icon ?? BRAND_ICON, brand: (size) => <HarnessLogo harness={h.id} logo={h.logo} size={size} /> };
}

export function providerMark(provider: OpenCodeProvider): AgentMark {
  const { tile } = providerTile(provider);
  return { icon: provider.icon ?? BRAND_ICON, brand: (size) => <TileMark tile={tile} size={size} /> };
}

export function endpointMark(connection: LocalModelConnection): AgentMark {
  const server = endpointServer(connection);
  return { icon: connection.icon ?? BRAND_ICON, brand: (size) => <EndpointMark server={server} size={size} /> };
}

export const FREE_MODELS_MARK: AgentMark = { icon: BRAND_ICON, brand: (size) => <HarnessLogo harness="opencode" size={size} /> };

/** A provider OpenCode signed in to itself, which no record of OpenResearch's names. */
function signInAgent(opencode: Harness, provider: string): PickerAgent {
  const { tile } = providerTile({ id: provider, name: provider });
  return { key: `opencode:${provider}`, name: tile.name, icon: BRAND_ICON, brand: (size) => <TileMark tile={tile} size={size} />, harness: opencode, provider };
}

/**
 * The agents that can run a model now, in Settings' order, less those turned off but `kept`. OpenCode stands
 * behind its providers.
 */
export function pickerAgents(harnesses: Harness[], providers: OpenCodeProvider[], endpoints: LocalModelConnection[], kept?: string): PickerAgent[] {
  const off = (h: Harness) => h.off && h.id !== kept;
  return sortHarnesses(harnesses).filter((h) => h.agentReady && (h.id === "opencode" || !off(h))).flatMap((h) => {
    if (h.id !== "opencode") return [{ key: harnessKey(h), name: h.profile?.name ?? harnessDisplayName(h), ...harnessMark(h), harness: h }];
    const used = new Set(h.models.map((model) => model.id.split("/")[0]));
    const records = [...providers].sort((a, b) => providerRank(a) - providerRank(b)).filter((provider) => used.has(provider.id));
    const connections = endpoints.filter((connection) => used.has(connection.id));
    const named = new Set([...records.map((provider) => provider.id), ...connections.map((connection) => connection.id), FREE_PROVIDER]);
    return [
      ...records.map((provider) => ({ key: `opencode:${provider.id}`, name: provider.name, ...providerMark(provider), harness: h, provider: provider.id })),
      ...connections.map((connection) => ({ key: `opencode:${connection.id}`, name: connection.name, ...endpointMark(connection), harness: h, provider: connection.id })),
      ...(used.has(FREE_PROVIDER) ? [{ key: `opencode:${FREE_PROVIDER}`, name: m.settings_free_models(), ...FREE_MODELS_MARK, harness: h, provider: FREE_PROVIDER }] : []),
      ...(off(h) ? [] : [...used].filter((provider) => !named.has(provider)).map((provider) => signInAgent(h, provider))),
    ];
  });
}

/** Which of `agents` runs `model` on harness entry `h`. */
export function agentFor(agents: PickerAgent[], h: Harness, model: string | null): PickerAgent | undefined {
  if (h.id !== "opencode") return agents.find((agent) => agent.key === harnessKey(h));
  const provider = model?.split("/")[0];
  return agents.find((agent) => agent.harness.id === "opencode" && agent.provider === provider);
}

/** An agent's models as offers; OpenCode's providers each offer their own. */
export function agentOffers(agent: PickerAgent): ModelOffer<PickerAgent>[] {
  const h = agent.harness;
  const models = agent.provider ? h.models.filter((model) => model.id.startsWith(`${agent.provider}/`)) : h.models;
  return models.map((model) => ({ tile: agent, model }));
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

  const agents = pickerAgents(harnesses, providers, endpoints, value?.harness);
  const offers = agents.flatMap(agentOffers);
  const selectedHarness = selectionHarness(harnesses, value);
  const selectedModel = value?.model ?? null;
  const selectedAgent = selectedHarness ? agentFor(agents, selectedHarness, selectedModel) : undefined;

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
