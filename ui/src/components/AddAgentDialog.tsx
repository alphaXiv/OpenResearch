import { useWorkspace } from "../workspace";
import { mutUpdateUiState } from "../queries/settings";
import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode, type Ref } from "react";
import { useQuery, type QueryClient } from "@tanstack/react-query";
import { ArrowLeft, Check, ChevronDown, CircleCheck, Copy, CreditCard, KeyRound, Laptop, Link2, Plus, RefreshCw, Search, TriangleAlert } from "lucide-react";
import {
  OpenCode1Error,
  harnessModelLabel,
  tidyModelName,
  type AcpDefinition,
  type AgentIcon,
  type Harness,
  type HarnessId,
  type HarnessModel,
  type HarnessSetupCommands,
  type OpenCodeAnswer,
  type OpenCodeFormField,
  type OpenCodeProvider,
  type OpenCodeSignIn,
} from "../api";
import type { DialogContentProps } from "../dialog";
import { copyToClipboard } from "../clipboard";
import { fmtNumber, ltr } from "../i18n";

import { m } from "../paraglide/messages.js";
import { agentModelLines } from "../modelSelection";
import { AcpAuth } from "./AcpAuthDialog";
import { AgentIconButton, BRAND_ICON } from "./AgentIcon";
import { BrandMark, HarnessLogo, ModelMark, type Brand } from "./HarnessLogo";
import { HarnessSetup, SignInSteps } from "./HarnessSetupDialog";
import { EndpointMark, LocalModelGuide, useLocalModelForm } from "./LocalModelSetup";
import { renderNote } from "./agentNote";
import { Button, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Input, Spinner, Switch } from "./ui";
import { cn } from "./ui/cn";

/** Each agent's own entry first, then the accounts OpenResearch keeps for it. */
export function sortHarnesses(harnesses: Harness[]) {
  const rank = (h: Harness) => (h.id === "claude-code" ? 0 : h.id === "codex" ? 1 : h.id === "opencode" ? 2 : 3) * 2 + (h.profile ? 1 : 0);
  return [...harnesses].sort((a, b) => rank(a) - rank(b));
}

export function canConfigureHarness(harness: Harness, remote: boolean, commands?: HarnessSetupCommands) {
  if (harness.needsConfigRepair) return false;
  if (harness.id.startsWith("acp:") && harness.installed && !harness.adapterMissing) return true;
  if (remote || !commands || (harness.logo === "zcode-acp-server" && !harness.installed && !harness.adapterMissing)) return false;
  return !harness.installed || harness.installBroken || harness.adapterMissing || harness.authState === "unsupported" || (harness.authState === "needsLogin" && (harness.id !== "claude-code" || harness.loginEligible));
}

/** OpenCode's version as the server reads it, from output like `opencode v2.2.5` or `1.18.35`. */
export function opencodeVersion(opencode: Harness | undefined) {
  return opencode?.version?.split(/\s+/).map((token) => token.replace(/^v/, "")).find((token) => /^\d+\.\d+\.\d+/.test(token));
}

/** OpenCode's major version, or 0 when it isn't there to ask. */
export function opencodeMajor(opencode: Harness | undefined) {
  return opencode?.installed && !opencode.installBroken ? Number(opencodeVersion(opencode)?.split(".")[0] ?? 0) : 0;
}

/** A key OpenResearch keeps for OpenCode; `id` is OpenCode's provider id. A `plan` key is a subscription's. */
type KeySignIn = { kind: "key"; id: string; platform: string; keyUrl?: string; plan?: string };
/** A sign-in through OpenCode's own API; without `method`, the provider's first sign-in method. `label` is OpenCode's name for it. */
type OAuthSignIn = { kind: "oauth"; integration: string; method?: string; plan?: string; label?: string };

/** One way to sign in. `profile` is a Claude Code or Codex account OpenResearch keeps; `agent` runs
 * that agent's own sign-in; `native` is a lab's own ACP agent, found by its logo, with `key` when it
 * takes the same API key; `opencode` is whichever ways OpenCode lists for one of its providers. */
type SignIn =
  | KeySignIn
  | OAuthSignIn
  | { kind: "opencode"; id: string }
  | { kind: "profile"; harness: "claude-code" | "codex" }
  | { kind: "agent"; harness: HarnessId; plan?: string; via?: string }
  | { kind: "native"; logo: string; key?: true }
  | { kind: "endpoint"; guide?: true };

type AgentTile = {
  id: string;
  name: string;
  mark: { brand: Brand } | { harness: HarnessId; logo?: string } | { icon: "endpoint" | "local" | "custom" } | { letter: string };
  blurb?: () => string;
  methods: SignIn[];
};

const CLAUDE: AgentTile = { id: "claude-code", name: "Claude Code", mark: { harness: "claude-code" }, blurb: m.add_agent_claude_blurb, methods: [{ kind: "profile", harness: "claude-code" }] };
const CODEX: AgentTile = { id: "codex", name: "OpenAI Codex", mark: { harness: "codex" }, blurb: m.add_agent_codex_blurb, methods: [{ kind: "profile", harness: "codex" }] };
const OPENCODE_GO: AgentTile = {
  id: "opencode-go", name: "OpenCode Go", mark: { harness: "opencode" }, blurb: m.add_agent_opencode_go_blurb,
  methods: [{ kind: "key", id: "opencode-go", platform: "OpenCode Go", keyUrl: "https://opencode.ai/console" }],
};
/** The labs' own models, in the order they're offered. */
const LABS: AgentTile[] = [
  {
    id: "deepseek", name: "DeepSeek", mark: { brand: "deepseek" },
    methods: [
      { kind: "key", id: "deepseek", platform: "DeepSeek", keyUrl: "https://platform.deepseek.com/api_keys" },
      { kind: "native", logo: "dsh", key: true },
    ],
  },
  {
    id: "kimi", name: "Kimi", mark: { brand: "kimi" },
    methods: [
      { kind: "key", id: "kimi-code-plan-global", plan: "Kimi Code membership", platform: "Kimi", keyUrl: "https://www.kimi.ai/code/console" },
      { kind: "key", id: "moonshotai", platform: "Moonshot", keyUrl: "https://platform.kimi.ai/console/api-keys" },
      { kind: "native", logo: "kimi" },
    ],
  },
  {
    id: "grok", name: "Grok", mark: { brand: "grok" },
    methods: [
      { kind: "oauth", integration: "xai", plan: "SuperGrok or X Premium+" },
      { kind: "key", id: "xai", platform: "xAI", keyUrl: "https://console.x.ai" },
      { kind: "native", logo: "grok" },
    ],
  },
  { id: "cursor", name: "Cursor", mark: { harness: "cursor" }, methods: [{ kind: "agent", harness: "cursor" }] },
  {
    id: "glm", name: "GLM", mark: { brand: "zai" },
    methods: [
      { kind: "key", id: "zai-coding-plan", plan: "GLM Coding Plan", platform: "Z.ai", keyUrl: "https://z.ai/subscribe" },
      { kind: "key", id: "zai", platform: "Z.ai", keyUrl: "https://z.ai/manage-apikey/apikey-list" },
      { kind: "native", logo: "zcode-acp-server" },
    ],
  },
  {
    id: "qwen", name: "Qwen", mark: { brand: "qwen" },
    methods: [{ kind: "key", id: "alibaba", platform: "Alibaba Cloud", keyUrl: "https://modelstudio.console.alibabacloud.com/ap-southeast-1/settings/api-key" }],
  },
  {
    id: "minimax", name: "MiniMax", mark: { brand: "minimax" },
    methods: [
      { kind: "key", id: "minimax-coding-plan", plan: "MiniMax M Plan", platform: "MiniMax", keyUrl: "https://platform.minimax.io/console/plan" },
      { kind: "key", id: "minimax", platform: "MiniMax", keyUrl: "https://platform.minimax.io/console/keys" },
      { kind: "native", logo: "mcode" },
    ],
  },
  {
    id: "gemini", name: "Gemini", mark: { brand: "gemini" },
    methods: [
      { kind: "agent", harness: "antigravity", plan: "Google AI plan", via: "Antigravity" },
      { kind: "key", id: "google", platform: "Google AI Studio", keyUrl: "https://aistudio.google.com/apikey" },
    ],
  },
];
const OPENROUTER: AgentTile = {
  id: "openrouter", name: "OpenRouter", mark: { brand: "openrouter" }, blurb: m.add_agent_openrouter_blurb,
  methods: [{ kind: "key", id: "openrouter", platform: "OpenRouter", keyUrl: "https://openrouter.ai/keys" }],
};
/** A provider from OpenCode's list, by its id there. */
const listedTile = (id: string, name: string, mark: AgentTile["mark"]): AgentTile => ({ id, name, mark, methods: [{ kind: "opencode", id }] });
/** Providers people look for by name, in the order offered; search finds the rest of OpenCode's list. */
const MORE_PROVIDERS = [
  listedTile("github-copilot", "GitHub Copilot", { brand: "github-copilot" }),
  listedTile("openai", "OpenAI", { harness: "codex" }),
  listedTile("anthropic", "Anthropic", { brand: "anthropic" }),
  listedTile("huggingface", "Hugging Face", { brand: "huggingface" }),
  listedTile("amazon-bedrock", "Amazon Bedrock", { brand: "bedrock" }),
  listedTile("groq", "Groq", { brand: "groq" }),
  listedTile("cerebras", "Cerebras", { brand: "cerebras" }),
  listedTile("togetherai", "Together AI", { brand: "together" }),
  listedTile("fireworks-ai", "Fireworks AI", { brand: "fireworks" }),
  listedTile("mistral", "Mistral", { brand: "mistral" }),
  listedTile("nvidia", "NVIDIA", { brand: "nvidia" }),
  listedTile("vercel", "Vercel AI Gateway", { brand: "vercel" }),
  listedTile("ollama-cloud", "Ollama Cloud", { brand: "ollama" }),
  listedTile("deepinfra", "DeepInfra", { brand: "deepinfra" }),
  listedTile("baseten", "Baseten", { brand: "baseten" }),
];
const PROVIDER_TILES = [OPENCODE_GO, ...LABS, OPENROUTER, ...MORE_PROVIDERS];
/** What the wizard recommends first. */
export const RECOMMENDED_TILES = [CLAUDE, CODEX, OPENCODE_GO];
/** The labs' own ACP agents, offered on their lab's tile rather than in the shelf. */
const NATIVE_LOGOS = new Set(LABS.flatMap((tile) => tile.methods.flatMap((method) => method.kind === "native" ? [method.logo] : [])));
/** Marks for providers no tile offers, by id; each also covers that id's regional and plan variants, like `alibaba-cn`. */
const PROVIDER_MARKS: Record<string, AgentTile["mark"]> = {
  moonshotai: { brand: "moonshot" }, kimi: { brand: "kimi" }, zhipuai: { brand: "zai" }, alibaba: { brand: "qwen" }, minimax: { brand: "minimax" }, opencode: { harness: "opencode" },
};

/** The OpenCode providers a tile signs in to. */
function providerIds(tile: AgentTile) {
  return tile.methods.flatMap((method) => method.kind === "key" || method.kind === "opencode" ? [method.id] : method.kind === "oauth" ? [method.integration] : []);
}

/** The tile a provider belongs to, and for a saved key the sign-in that made it. */
export function providerTile(provider: Pick<OpenCodeProvider, "id" | "canonical" | "name"> & Partial<Pick<OpenCodeProvider, "kind">>): { tile: AgentTile; key?: KeySignIn } {
  const id = provider.canonical ?? provider.id;
  const tile = PROVIDER_TILES.find((candidate) => providerIds(candidate).includes(id));
  if (!tile) {
    const variant = Object.entries(PROVIDER_MARKS).find(([base]) => id === base || id.startsWith(`${base}-`));
    return { tile: listedTile(id, provider.name, variant?.[1] ?? { letter: provider.name.slice(0, 1).toUpperCase() }) };
  }
  return { tile, key: provider.kind === "key" ? tile.methods.find((method): method is KeySignIn => method.kind === "key" && method.id === id) : undefined };
}

/** Where saved providers sort: the order the add dialog offers them. */
export function providerRank(provider: Pick<OpenCodeProvider, "id" | "canonical">) {
  const id = provider.canonical ?? provider.id;
  const index = PROVIDER_TILES.findIndex((tile) => providerIds(tile).includes(id));
  return index < 0 ? Number.MAX_SAFE_INTEGER : index;
}

/** Claude Code's or Codex's tile, for the accounts OpenResearch keeps. */
export function profileTile(harness: HarnessId) {
  return harness === "codex" ? CODEX : CLAUDE;
}

/** `base`, or `base 2`, `base 3`… when an added agent already has that name. */
function freeName(base: string, taken: string[]) {
  const names = new Set(taken.map((name) => name.toLowerCase()));
  let name = base;
  for (let n = 2; names.has(name.toLowerCase()); n++) name = `${base} ${n}`;
  return name;
}

/** A provider key's field, then where to get one. `name` stands in for a key no sign-in describes. */
export function KeyField({ method, name, value, disabled, onChange, inputRef }: {
  method?: KeySignIn;
  name: string;
  value: string;
  disabled: boolean;
  onChange: (value: string) => void;
  inputRef?: Ref<HTMLInputElement>;
}) {
  return (
    <div className="flex flex-col gap-2">
      <label className="grid gap-1.5 text-sm text-subtext">
        <span>{method?.plan ? m.add_agent_plan_key_label({ plan: method.plan }) : m.provider_key_label({ name: method?.platform ?? name })}</span>
        <Input ref={inputRef} type="password" autoComplete="off" spellCheck={false} value={value} disabled={disabled} onChange={(event) => onChange(event.target.value)} />
      </label>
      {method?.keyUrl && <a href={method.keyUrl} target="_blank" rel="noreferrer" className="self-start text-sm text-accent-blue-strong underline">{m.provider_key_get()}</a>}
    </div>
  );
}

/** Subscription or API key, for a tile that offers both. */
export function keyKind(method: KeySignIn) {
  return method.plan ? m.add_agent_subscription() : m.add_agent_api_key();
}

export function TileMark({ tile, size }: { tile: AgentTile; size: number }) {
  if ("brand" in tile.mark) return <BrandMark brand={tile.mark.brand} size={size} />;
  if ("harness" in tile.mark) return <HarnessLogo harness={tile.mark.harness} logo={tile.mark.logo} size={size} />;
  if ("letter" in tile.mark) {
    return <span aria-hidden="true" className="grid shrink-0 place-items-center font-semibold leading-none" style={{ width: size, height: size, fontSize: size * 0.75 }}>{tile.mark.letter}</span>;
  }
  if (tile.mark.icon === "custom") return <Plus size={size} className="shrink-0" aria-hidden="true" />;
  if (tile.mark.icon === "local") return <Laptop size={size} className="shrink-0" aria-hidden="true" />;
  return <Link2 size={size} className="shrink-0" aria-hidden="true" />;
}

type Step = "provider" | "signin" | "name";
const STEPS: Step[] = ["provider", "signin", "name"];

/** What this dialog made, which leaving before the end removes. A vendor's own sign-in stays with
 * its agent, so it never appears here. `name` is what it was made with; `replaced` marks an OpenCode
 * sign-in or endpoint model the user already had, which leaving keeps. */
type Saved =
  | { kind: "profile"; id: string; name: string }
  | { kind: "provider"; id: string; name: string; replaced?: boolean }
  | { kind: "endpoint"; id: string; model: string; name: string; replaced: boolean };

/** Which agent the wizard added; Claude Code and Codex name the account, OpenCode the provider of the models it added. */
export type AddedAgent = { harness: HarnessId; profile?: string; provider?: string };

export function AddAgentDialog({ remote, host, tile: opening, hideRecommended = false, confirm, cancel, isDisabled, onCustomAcp, onEditAcp }: DialogContentProps<AddedAgent> & {
  remote: boolean;
  /** A recommended tile's id, or `endpoint`, whose sign-in the wizard opens on. */
  tile?: string;
  /** Leaves out the recommended agents, for a page that offers them already. */
  hideRecommended?: boolean;
  /** The remote machine being set up, where a Claude subscription signed in from a datacenter IP may be flagged. */
  host?: string;
  onCustomAcp: () => void;
  onEditAcp: (definition: AcpDefinition) => void;
}) {
  const workspace = useWorkspace();
  const { getHarnessesQuery, getHarnessSetupCommandsQuery, getOpenCodeProvidersQuery, getLocalModelsQuery, getUiStateQuery, listAcpHarnessesQuery, refreshHarness } = workspace.queries;
  const { workspaceKey, api } = workspace;
  const { showCustomDialog } = workspace.dialogs;

  const harnessQuery = useQuery(getHarnessesQuery());
  const definitions = useQuery(listAcpHarnessesQuery());
  const presets = useQuery({ queryKey: workspaceKey("getAcpPresets"), queryFn: api.listAcpPresets });
  const setupCommands = useQuery(getHarnessSetupCommandsQuery());
  const providers = useQuery(getOpenCodeProvidersQuery());
  const endpoints = useQuery(getLocalModelsQuery());
  const uiState = useQuery(getUiStateQuery());
  const [step, setStep] = useState<Step>("provider");
  const [tile, setTile] = useState<AgentTile | null>(null);
  const [method, setMethod] = useState<SignIn | null>(null);
  const [key, setKey] = useState("");
  const [saved, setSaved] = useState<Saved | null>(null);
  const [signedIn, setSignedIn] = useState(false);
  const [oauth, setOauth] = useState<(OpenCodeSignIn & { replaced: boolean }) | null>(null);
  /** What OpenCode asks before a sign-in starts, such as GitHub's deployment type. */
  const [questions, setQuestions] = useState<{ signIn: OAuthSignIn; from: AgentTile; fields: OpenCodeFormField[] } | null>(null);
  const [answers, setAnswers] = useState<Record<string, OpenCodeAnswer>>({});
  /** A request OpenCode 1 refused, until it's updated. */
  const [refused, setRefused] = useState(false);
  const [name, setName] = useState("");
  /** Null until the user picks one: the tile's mark on its colour. */
  const [icon, setIcon] = useState<AgentIcon | null>(null);
  const [hideFreeModels, setHideFreeModels] = useState(true);
  const [checking, setChecking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState(false);
  /** The local model guide found a server, so its form takes over. */
  const [localFound, setLocalFound] = useState(false);
  const warnFirst = useDatacenterWarning(host);
  const endpoint = useLocalModelForm(isDisabled);
  const bodyRef = useRef<HTMLDivElement>(null);
  const formId = useId();
  const moreId = useId();
  const harnesses = sortHarnesses(harnessQuery.data ?? []);
  const opencode = harnesses.find((h) => h.id === "opencode");
  const acpAvailable = definitions.data !== undefined && !definitions.isError;
  const disabled = busy || isDisabled;

  const opencodeReady = opencodeMajor(opencode) >= 2;
  const integrations = useQuery({
    // Keyed by version, so an updated OpenCode answers afresh.
    queryKey: workspaceKey("getOpenCodeIntegrations", opencode?.version),
    queryFn: ({ signal }) => api.getOpenCodeIntegrations(signal),
    enabled: opencodeReady && (expanded || method?.kind === "opencode" || method?.kind === "oauth"),
    staleTime: 60_000,
  });
  const needsOpenCode2 = refused || opencodeMajor(opencode) === 1 || integrations.error instanceof OpenCode1Error;
  const listing = tile?.methods.find((signIn) => signIn.kind === "opencode");
  const entry = listing && integrations.data?.find((item) => item.id === listing.id);
  // How OpenCode signs in to a provider from its list, sign-ins before keys.
  const ways: (OAuthSignIn | KeySignIn)[] = entry && tile ? [
    ...entry.methods.flatMap((item): OAuthSignIn[] => item.type === "oauth" && !item.label.endsWith("(legacy)") ? [{ kind: "oauth", integration: entry.id, method: item.id, label: item.label }] : []),
    ...entry.methods.flatMap((item): KeySignIn[] => item.type === "key" ? [{ kind: "key", id: entry.id, platform: tile.name }] : []),
  ] : [];

  const harnessFor = (signIn: SignIn) =>
    signIn.kind === "agent" || signIn.kind === "profile" ? harnesses.find((h) => h.id === signIn.harness && !h.profile)
      : signIn.kind === "native" ? harnesses.find((h) => h.logo === signIn.logo)
        : undefined;
  // A sign-in this server cannot offer drops off its tile, and a tile with none left goes.
  const offered = (candidate: AgentTile): AgentTile | null => {
    const methods = candidate.methods.filter((signIn) => !(signIn.kind === "agent" || signIn.kind === "profile" || signIn.kind === "native") || harnessFor(signIn));
    return methods.length ? { ...candidate, methods } : null;
  };
  const shelf: AgentTile[] = (presets.data ?? []).flatMap((preset) => {
    const harness = harnesses.find((h) => h.setupUrl === preset.setupUrl);
    if (!harness || (harness.logo && NATIVE_LOGOS.has(harness.logo))) return [];
    return [{ id: harness.id, name: preset.name, mark: { harness: harness.id, logo: harness.logo }, methods: [{ kind: "agent", harness: harness.id }] }];
  });
  const endpointTile: AgentTile = {
    id: "endpoint", name: m.local_models_custom_endpoint(), mark: { icon: "endpoint" }, blurb: m.add_agent_endpoint_blurb, methods: [{ kind: "endpoint" }],
  };
  const localTile: AgentTile = {
    id: "local", name: m.local_models_guide_name(), mark: { icon: "local" }, blurb: m.local_models_guide_blurb, methods: [{ kind: "endpoint", guide: true }],
  };
  const customAcp: AgentTile = { id: "custom-acp", name: m.harness_custom_acp(), mark: { icon: "custom" }, methods: [] };

  const methodHarness = method ? harnessFor(method) : undefined;
  const backedByOpenCode = method?.kind === "key" || method?.kind === "oauth" || method?.kind === "opencode" || method?.kind === "endpoint";
  const profileEntry = saved?.kind === "profile" ? harnesses.find((h) => h.profile?.id === saved.id) : undefined;
  const modelsReady = !checking && (backedByOpenCode ? opencodeReady && !opencode?.catalogPending : Boolean((profileEntry ?? methodHarness)?.agentReady));
  // Names the user would see twice: every added agent's, and the agents' own sign-ins Settings lists
  // (counted while a re-check can't yet say).
  const takenNames = (except?: string) => [
    ...harnesses.flatMap((h) => h.profile ? [h.profile.name] : (h.id === "claude-code" || h.id === "codex") && (h.agentReady || h.authenticated || h.catalogPending) ? [h.label?.name ?? h.name] : []),
    ...(providers.data ?? []).filter((provider) => provider.id !== except).map((provider) => provider.name),
    ...(endpoints.data ?? []).filter((connection) => connection.id !== except).map((connection) => connection.name),
  ];

  useEffect(() => {
    const target = bodyRef.current?.querySelector<HTMLElement>("input:not([type=hidden]), [data-autofocus]") ?? bodyRef.current;
    target?.focus();
  }, [step, method, ways.length, needsOpenCode2]);

  // Saved keys and endpoints reach OpenCode's catalog only once it re-checks.
  useEffect(() => {
    if (step !== "name" || !backedByOpenCode || !opencodeReady) return;
    let current = true;
    setChecking(true);
    void refreshHarness("opencode").catch(() => {}).finally(() => { if (current) setChecking(false); });
    return () => { current = false; };
  }, [step, opencodeReady]);

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try { await action(); }
    catch (cause) {
      if (cause instanceof OpenCode1Error) setRefused(true);
      else setError(cause instanceof Error ? cause.message : String(cause));
    }
    finally { setBusy(false); }
  };

  /** Claude Code and Codex always sign in to an account of OpenResearch's own. */
  const createProfile = (signIn: Extract<SignIn, { kind: "profile" }>, from: AgentTile) => run(async () => {
    // Named as the agent is elsewhere, so a second Codex account reads "Codex 2" rather than its tile's "OpenAI Codex".
    const agentName = harnesses.find((h) => h.id === signIn.harness && !h.profile)?.name ?? from.name;
    const profileName = freeName(agentName, takenNames());
    const { id } = await api.createAgentProfile({ harness: signIn.harness, name: profileName });
    setSaved({ kind: "profile", id, name: profileName });
    setName(profileName);
  });
  const startOAuth = (signIn: OAuthSignIn, from: AgentTile, answer?: Record<string, OpenCodeAnswer>) => run(async () => {
    const known = integrations.data ?? await api.getOpenCodeIntegrations();
    const target = known.find((item) => item.id === signIn.integration);
    const found = target?.methods.find((item) => item.type === "oauth" && (!signIn.method || item.id === signIn.method));
    const methodId = signIn.method ?? (found?.type === "oauth" ? found.id : undefined);
    if (!methodId) throw new Error(m.add_agent_oauth_unavailable({ name: from.name }));
    const fields = found?.type === "oauth" ? found.form : undefined;
    if (fields?.length && !answer) {
      setAnswers(Object.fromEntries(fields.flatMap((field) => field.default === undefined ? [] : [[field.key, field.default]])));
      setQuestions({ signIn, from, fields });
      return;
    }
    const recordName = freeName(from.name, takenNames(signIn.integration));
    setOauth({ ...await api.startOpenCodeSignIn({ integration: signIn.integration, method: methodId, name: recordName, ...(answer ? { answer } : {}) }), replaced: Boolean(target?.connected) });
    setName(recordName);
  });
  /** What a sign-in does first, once OpenCode can take it. */
  const begin = (signIn: SignIn | null, from: AgentTile, canSignIn = opencodeReady && !needsOpenCode2) => {
    if (signIn?.kind === "profile" && !remote) {
      const create = () => createProfile(signIn, from);
      void (signIn.harness === "claude-code" ? warnFirst(create, () => leave("provider")) : create());
    }
    if (signIn?.kind === "oauth" && canSignIn) void startOAuth(signIn, from);
  };
  const choose = (signIn: SignIn | null, from = tile) => {
    setMethod(signIn);
    setLocalFound(false);
    setKey("");
    setSignedIn(false);
    setOauth(null);
    setQuestions(null);
    setError(null);
    if (from) begin(signIn, from);
  };
  const pick = (next: AgentTile) => {
    if (next.id === "custom-acp") { cancel(); onCustomAcp(); return; }
    setTile(next);
    choose(next.methods.length === 1 ? next.methods[0] : null, next);
    setStep("signin");
  };
  // Once, though development renders mount twice: picking a tile may start a sign-in.
  const opened = useRef(false);
  useEffect(() => {
    const start = opening && [...RECOMMENDED_TILES, endpointTile].find((candidate) => candidate.id === opening);
    const shown = start && offered(start);
    if (opened.current || !shown) return;
    opened.current = true;
    if (step === "provider") pick(shown);
  }, [harnessQuery.data]);
  // A provider OpenCode signs in to only one way starts it as soon as OpenCode lists it.
  useLayoutEffect(() => {
    if (method?.kind === "opencode" && ways.length === 1) choose(ways[0]);
  }, [method, ways.length]);
  const discard = async () => {
    if (oauth && !signedIn) await api.cancelOpenCodeSignIn(oauth.attempt).catch(() => {});
    if (saved?.kind === "profile") await api.removeAgentProfile(saved.id);
    if (saved?.kind === "provider" && !saved.replaced) await api.removeOpenCodeProvider(saved.id);
    if (saved?.kind === "endpoint" && !saved.replaced) await api.removeLocalModel(saved.id, saved.model);
    setSaved(null);
    setOauth(null);
  };
  const discards = (target: Step | "choices" | "close") => target !== "name" && target !== "signin";
  const go = async (target: Step | "choices" | "close") => {
    setError(null);
    if (discards(target)) await discard();
    // Opened on a tile, the wizard's provider step is the page that opened it.
    if (target === "close" || (target === "provider" && opening)) { cancel(); return; }
    if (target === "choices") { choose(listing ?? null); return; }
    if (target === "provider") { setTile(null); choose(null); }
    setStep(target);
  };
  const leave = (target: Step | "choices" | "close") => {
    // Only what the user signed in to is worth a confirm.
    if (!signedIn || !saved || !discards(target)) { go(target).catch((cause: unknown) => setError(cause instanceof Error ? cause.message : String(cause))); return; }
    showCustomDialog({ component: DiscardDialog, props: { trigger: target === "close" ? "close" : "back" }, onConfirm: () => go(target), onCancel: "close" });
  };
  const back = () => {
    if (step === "signin" && method?.kind === "endpoint" && endpoint.back()) return;
    if (step === "name") leave("signin");
    else if (method && method.kind !== "opencode" && (listing ? ways : tile?.methods ?? []).length > 1) leave("choices");
    else leave("provider");
  };

  const saveKey = (signIn: KeySignIn) => run(async () => {
    // Coming back to change the key replaces the one this dialog saved.
    if (saved?.kind === "provider") await api.replaceOpenCodeProviderKey(saved.id, key.trim());
    else {
      const recordName = freeName(tile?.name ?? signIn.platform, takenNames());
      const { id } = await api.addOpenCodeProvider({ provider: signIn.id, apiKey: key.trim(), name: recordName });
      setSaved({ kind: "provider", id, name: recordName });
      setName(recordName);
    }
    setSignedIn(true);
    setStep("name");
  });
  const connectEndpoint = () => run(async () => {
    const request = endpoint.request;
    if (!request) return;
    const { model, created } = await api.connectLocalModel(request);
    const id = model.slice(0, model.indexOf("/"));
    const previous = saved?.kind === "endpoint" ? saved : null;
    const same = previous?.id === id && previous.model === request.model;
    // Coming back to change the model replaces the one this dialog saved.
    if (previous && !same && !previous.replaced) await api.removeLocalModel(previous.id, previous.model);
    const connectionName = endpoints.data?.find((connection) => connection.id === id)?.name ?? request.name;
    const recordName = previous?.id === id ? name : freeName(connectionName, takenNames(id));
    setSaved({ kind: "endpoint", id, model: request.model, name: connectionName, replaced: previous && same ? previous.replaced : !created });
    setName(recordName);
    setSignedIn(true);
    setStep("name");
  });
  const finish = () => run(async () => {
    const trimmed = name.trim();
    if (saved && (trimmed !== saved.name || icon)) {
      const changes = { ...(trimmed !== saved.name ? { name: trimmed } : {}), ...(icon ? { icon } : {}) };
      await api.updateAgent(saved.kind === "profile" ? saved.id : `opencode:${saved.id}`, changes);
    }
    const hiddenAgents = uiState.data?.hiddenAgents ?? [];
    const turnedOn = method?.kind === "agent" || method?.kind === "native" ? methodHarness?.id : undefined;
    if (turnedOn && hiddenAgents.includes(turnedOn)) mutUpdateUiState.run(workspace, { hiddenAgents: hiddenAgents.filter((id) => id !== turnedOn) });
    if (tile?.id === "opencode-go" && uiState.data?.opencodeFreeModels && hideFreeModels)
      mutUpdateUiState.runWithOptions(workspace, { opencodeFreeModels: false }, { onSuccessData: () => void refreshHarness("opencode").catch(() => {}) });
    confirm(saved?.kind === "profile" && method?.kind === "profile" ? { harness: method.harness, profile: saved.id }
      : backedByOpenCode ? { harness: "opencode", ...(saved ? { provider: saved.id } : {}) }
        : { harness: methodHarness?.id ?? "opencode" });
  });

  const search = query.trim().toLowerCase();
  const recommended = RECOMMENDED_TILES.flatMap((candidate) => offered(candidate) ?? []);
  const labs = LABS.flatMap((candidate) => offered(candidate) ?? []);
  const routes = [OPENROUTER, localTile, endpointTile];
  // A tile also answers to the providers it signs in to, so "moonshot" finds Kimi.
  const matches = (candidate: AgentTile) => [
    candidate.id,
    candidate.name,
    ...candidate.methods.flatMap((signIn) => signIn.kind === "key" ? [signIn.platform] : []),
    ...providerIds(candidate).flatMap((id) => [id, integrations.data?.find((item) => item.id === id)?.name ?? ""]),
  ].some((term) => term.toLowerCase().includes(search));
  const known = [...recommended, ...labs, ...routes, ...MORE_PROVIDERS];
  const later = (candidate: AgentTile) => Number(!candidate.name.toLowerCase().startsWith(search));
  // Each provider shows once: as the tile that offers it, else as OpenCode lists it.
  const providerMatches = search ? [
    ...known.filter(matches),
    ...(integrations.data ?? [])
      .filter((item) => !known.some((candidate) => providerIds(candidate).includes(item.id)))
      .map((item) => providerTile(item).tile)
      .filter(matches)
      .sort((a, b) => later(a) - later(b) || a.name.localeCompare(b.name)),
  ] : MORE_PROVIDERS;
  const agentMatches = [...shelf, customAcp].filter(matches);
  const choice = (candidate: AgentTile, featured = false): Choice => ({
    key: candidate.id,
    name: candidate.name,
    blurb: featured || candidate.methods.length === 1 ? candidate.blurb?.() : undefined,
    mark: <TileMark tile={candidate} size={24} />,
    unavailable: (candidate.id === "custom-acp" || candidate.id.startsWith("acp:")) && !acpAvailable,
    select: () => pick(candidate),
  });

  const recordless = method !== null && (method.kind === "agent" || method.kind === "native");
  const stepLabels = [
    m.add_agent_step_provider(),
    tile?.methods[0]?.kind === "endpoint" ? m.add_agent_step_endpoint() : m.add_agent_step_sign_in(),
    recordless ? m.add_agent_step_models() : m.add_agent_step_name(),
  ];
  const title = step === "provider" || !tile ? m.harness_add()
    : step === "name" ? (recordless ? m.add_agent_models_title({ name: tile.name }) : m.add_agent_name_title())
      : tile.id === "local" ? m.local_models_guide_title()
      : tile.id === "endpoint" ? m.local_models_add()
        : m.add_agent_sign_in_title({ name: tile.name });
  const logo = (size: number) => tile?.id === "endpoint" || (tile?.id === "local" && localFound) ? <EndpointMark server={endpoint.server} size={size} /> : tile && <TileMark tile={tile} size={size} />;

  /** OpenCode 2 is there to sign in with, or what it takes to get it. */
  const opencodeGate = (): ReactNode => {
    if (opencodeReady && !needsOpenCode2) return null;
    const commands = setupCommands.data?.opencode;
    if (remote || !opencode || !commands) return <p className="m-0 text-sm text-subtext">{m.settings_needs_opencode()}</p>;
    const ready = () => { setRefused(false); if (tile) begin(method, tile, true); };
    if (needsOpenCode2) return <OpenCodeUpdate key="opencode-update" opencode={opencode} commands={commands} disabled={isDisabled} onReady={ready} />;
    return <HarnessSetup key="opencode-install" harness={opencode} commands={commands} action="install" disabled={isDisabled} onReady={ready} />;
  };

  const gate = step === "signin" && backedByOpenCode ? opencodeGate() : null;
  let body: ReactNode;
  let primary: ReactNode = null;
  /** Starts again what failed before the step had anything to show. */
  let retry: (() => void) | null = null;
  if (step === "provider" || !tile) {
    body = <>
      {(definitions.isError || presets.isError) && (
        <p role="alert" className="mt-0 mb-4 text-sm text-subtext">
          {m.acp_definitions_load_failed()}{" "}
          <Button variant="ghost" onClick={() => { void definitions.refetch(); void presets.refetch(); }}>{m.app_retry()}</Button>
        </p>
      )}
      {!hideRecommended && <>
        <h3 className="mt-0 mb-2 text-sm font-medium text-subtext">{m.add_agent_recommended()}</h3>
        <ChoiceGrid choices={recommended.map((candidate) => choice(candidate, true))} columns={3} featured />
      </>}
      <h3 className={cn("mb-2 text-sm font-medium text-subtext", hideRecommended ? "mt-0" : "mt-5")}>{m.add_agent_other_providers()}</h3>
      <ChoiceGrid choices={labs.map((candidate) => choice(candidate))} columns={4} />
      <div className="mt-2"><ChoiceGrid choices={routes.map((candidate) => choice(candidate))} columns={3} /></div>
      <h3 className="mt-5 mb-0">
        <button
          type="button"
          aria-expanded={expanded}
          aria-controls={moreId}
          onClick={() => setExpanded((open) => !open)}
          className="flex w-full items-center gap-2 rounded-md py-1 text-start text-sm font-medium text-subtext outline-none hover:text-text focus-visible:outline-2 focus-visible:outline-focus"
        >
          <ChevronDown size={14} className={cn("transition-transform", !expanded && "-rotate-90")} aria-hidden="true" />
          {m.add_agent_more()}
        </button>
      </h3>
      <div id={moreId} hidden={!expanded} className="mt-2">
        {expanded && <>
          <SearchField label={m.add_agent_search()} value={query} onChange={setQuery} />
          <div className="grid gap-4">
            {providerMatches.length > 0 && <div>
              <h4 className="mt-0 mb-2 text-xs font-medium text-subtext">{m.add_agent_more_providers()}</h4>
              <ChoiceGrid choices={providerMatches.map((candidate) => choice(candidate))} columns={3} />
            </div>}
            {search && integrations.isLoading && (
              <p role="status" className="m-0 flex items-center gap-2 text-sm text-subtext"><Spinner />{m.add_agent_providers_loading()}</p>
            )}
            {search && integrations.isError && !integrations.data && (
              <p role="alert" className="m-0 text-sm text-subtext">
                {m.add_agent_providers_failed()}{" "}
                <Button variant="ghost" onClick={() => void integrations.refetch()}>{m.app_retry()}</Button>
              </p>
            )}
            {agentMatches.length > 0 && <div>
              <h4 className="mt-0 mb-2 text-xs font-medium text-subtext">{m.add_agent_more_agents()}</h4>
              <ChoiceGrid choices={agentMatches.map((candidate) => choice(candidate))} columns={3} />
            </div>}
            {!providerMatches.length && !agentMatches.length && !integrations.isLoading && (
              <p role="status" className="m-0 text-sm text-subtext">{m.add_agent_no_matches({ query: query.trim() })}</p>
            )}
          </div>
        </>}
      </div>
    </>;
  } else if (step === "signin" && !method) {
    // Where the lab's own agent stands beside one OpenCode way in, the choice is which agent runs it.
    const versusAgent = tile.methods.some((signIn) => signIn.kind === "native") && tile.methods.filter((signIn) => signIn.kind === "key" || signIn.kind === "oauth").length === 1;
    body = <MethodList methods={tile.methods.map((signIn) => {
      const harness = harnessFor(signIn);
      return signIn.kind === "key" && versusAgent ? { label: m.add_agent_opencode_recommended(), detail: signIn.plan ?? m.add_agent_key_with({ platform: signIn.platform }), mark: <HarnessLogo harness="opencode" size={20} />, select: () => choose(signIn) }
        : signIn.kind === "key" ? { label: keyKind(signIn), detail: signIn.plan ?? m.add_agent_api_key_detail({ platform: signIn.platform }), mark: signIn.plan ? <CreditCard size={20} /> : <KeyRound size={20} />, select: () => choose(signIn) }
        : signIn.kind === "native" ? { label: harness?.name ?? "", detail: signIn.key ? m.add_agent_native_key_detail({ lab: tile.name }) : m.add_agent_native_detail({ lab: tile.name }), mark: harness && <HarnessLogo harness={harness.id} logo={harness.logo} size={20} />, select: () => choose(signIn) }
          : (signIn.kind === "agent" || signIn.kind === "oauth") && signIn.plan ? { label: m.add_agent_subscription(), detail: signIn.kind === "agent" && signIn.via ? m.add_agent_subscription_in({ plan: signIn.plan, agent: signIn.via }) : signIn.plan, mark: <CreditCard size={20} />, select: () => choose(signIn) }
            : { label: harness?.name ?? "", detail: "", mark: null, select: () => choose(signIn) };
    })} disabled={disabled} />;
  } else if (gate) {
    body = gate;
  } else if (step === "signin" && method?.kind === "opencode") {
    body = integrations.isError && !integrations.data ? (
      <p role="alert" className="m-0 flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-accent-red">
        {integrations.error.message}
        <Button size="small" onClick={() => void integrations.refetch()} disabled={disabled}>{m.app_retry()}</Button>
      </p>
    ) : ways.length > 1 ? (
      <MethodList methods={ways.map((way) => way.kind === "oauth"
        ? { label: way.label ?? "", detail: "", mark: <CreditCard size={20} />, select: () => choose(way) }
        : { label: m.add_agent_api_key(), detail: m.add_agent_api_key_detail({ platform: way.platform }), mark: <KeyRound size={20} />, select: () => choose(way) })} disabled={disabled} />
    ) : (
      <p role="status" className="m-0 flex items-center gap-2 text-sm text-subtext">
        {!integrations.data || ways.length ? <><Spinner />{m.harness_setup_signin_starting()}</> : m.add_agent_provider_missing({ name: tile.name })}
      </p>
    );
  } else if (step === "signin" && method?.kind === "key") {
    body = <form id={formId} className="flex flex-col gap-4" onSubmit={(event) => {
      event.preventDefault();
      if (disabled) return;
      if (key.trim()) void saveKey(method);
      else if (saved) setStep("name");
    }}>
      {saved?.kind === "provider" && <SignedIn />}
      <KeyField method={method} name={tile.name} value={key} disabled={disabled} onChange={setKey} />
    </form>;
    primary = <Button type="submit" form={formId} variant="primary" disabled={(!key.trim() && !saved) || disabled}>{busy && <Spinner />}{m.onboarding_continue()}</Button>;
  } else if (step === "signin" && method?.kind === "oauth" && questions && !oauth) {
    const shown = askedFields(questions.fields, answers);
    const answered = Object.fromEntries(shown.flatMap((field) => answers[field.key] === undefined || answers[field.key] === "" ? [] : [[field.key, answers[field.key]]]));
    body = <SignInQuestions formId={formId} fields={shown} answers={answers} disabled={disabled}
      onAnswer={(key, value) => setAnswers((current) => ({ ...current, [key]: value }))}
      onSubmit={() => void startOAuth(questions.signIn, questions.from, answered)} />;
    primary = <Button type="submit" form={formId} variant="primary" disabled={shown.some((field) => field.required && answered[field.key] === undefined) || disabled}>
      {busy && <Spinner />}{m.onboarding_continue()}
    </Button>;
  } else if (step === "signin" && method?.kind === "oauth") {
    body = signedIn ? <SignedIn /> : oauth ? (
      <OAuthPanel
        key={oauth.attempt}
        signIn={oauth}
        disabled={disabled}
        onComplete={() => {
          setSaved({ kind: "provider", id: method.integration, name, replaced: oauth.replaced });
          setSignedIn(true);
          setStep("name");
        }}
        onRetry={() => void startOAuth(method, tile)}
      />
    ) : busy ? (
      <p role="status" className="m-0 flex items-center gap-2 text-sm text-subtext"><Spinner />{m.harness_setup_signin_starting()}</p>
    ) : null;
    retry = oauth || signedIn ? null : () => void startOAuth(method, tile);
    primary = <Button variant="primary" disabled={!signedIn || disabled} onClick={() => setStep("name")}>{m.onboarding_continue()}</Button>;
  } else if (step === "signin" && method?.kind === "endpoint" && method.guide && !localFound) {
    body = <LocalModelGuide disabled={disabled} onFound={(found) => { endpoint.adopt(found); setLocalFound(true); }} />;
  } else if (step === "signin" && method?.kind === "endpoint") {
    body = <form id={formId} onSubmit={(event) => {
      event.preventDefault();
      if (endpoint.canSubmit && !disabled) void (endpoint.found ? connectEndpoint() : endpoint.check());
    }}>
      {endpoint.fields}
      {endpoint.error && <p className="mt-4 mb-0 text-sm text-accent-red" role="alert">{endpoint.error}</p>}
    </form>;
    primary = <Button type="submit" form={formId} variant="primary" disabled={!endpoint.canSubmit || disabled}>
      {(busy || endpoint.busy) && <Spinner />}{endpoint.found ? m.onboarding_continue() : m.local_models_check()}
    </Button>;
  } else if (step === "signin" && method?.kind === "profile" && methodHarness) {
    const commands = setupCommands.data?.[methodHarness.id];
    const starting = <p role="status" className="m-0 flex items-center gap-2 text-sm text-subtext"><Spinner />{m.harness_setup_signin_starting()}</p>;
    body = signedIn ? <SignedIn />
      : remote || setupCommands.isError ? <AgentSignIn harness={methodHarness} remote={remote} disabled={isDisabled} onReady={() => {}} onEdit={() => {}} />
        : !commands ? starting
        : saved?.kind === "profile" ? (
          <HarnessSetup
            key={saved.id}
            harness={methodHarness}
            commands={commands}
            // Installing, updating, or repairing the agent comes first; the sign-in is the account's.
            action={methodHarness.installed && !methodHarness.installBroken && methodHarness.authState !== "unsupported" ? "login" : undefined}
            profile={saved.id}
            disabled={isDisabled}
            onReady={() => { setSignedIn(true); setStep("name"); }}
          />
        ) : busy ? starting : null;
    retry = saved ? null : () => void createProfile(method, tile);
    primary = <Button variant="primary" disabled={!signedIn || disabled} onClick={() => setStep("name")}>{m.onboarding_continue()}</Button>;
  } else if (step === "signin" && method && methodHarness) {
    // An agent that already works on this computer has nothing left to set up.
    const ready = signedIn || (methodHarness.agentReady && !methodHarness.authCheckFailed);
    body = ready
      ? <SignedIn />
      : <AgentSignIn
          key={methodHarness.id}
          harness={methodHarness}
          commands={setupCommands.data?.[methodHarness.id]}
          remote={remote}
          disabled={isDisabled}
          definition={definitions.data?.find((item) => item.id === methodHarness.id)}
          formId={formId}
          onReady={() => { setSignedIn(true); setStep("name"); }}
          onEdit={(definition) => { cancel(); onEditAcp(definition); }}
        />;
    // An installed agent that takes a key: Continue saves it and checks the agent.
    const keyForm = !ready && !remote && methodHarness.id.startsWith("acp:") && methodHarness.installed && !methodHarness.adapterMissing && Boolean(methodHarness.authEnvKey);
    primary = keyForm
      ? <Button type="submit" form={formId} variant="primary" disabled={disabled}>{m.onboarding_continue()}</Button>
      : <Button variant="primary" disabled={!ready || disabled} onClick={() => setStep("name")}>{m.onboarding_continue()}</Button>;
  } else if (step === "name") {
    const listed: HarnessModel[] =
      saved?.kind === "provider" ? opencode?.models.filter((model) => model.id.startsWith(`${saved.id}/`)) ?? []
        : saved?.kind === "endpoint" ? opencode?.models.filter((model) => model.id === `${saved.id}/${saved.model}`) ?? []
          : (profileEntry ?? methodHarness)?.models ?? [];
    body = <div className="space-y-5">
      {saved && (
        <div className="flex items-end gap-3">
          <AgentIconButton value={icon ?? BRAND_ICON} brand={logo} onChange={setIcon} disabled={disabled} />
          <label className="grid flex-1 gap-1 text-sm text-subtext">
            <span>{m.agent_name()}</span>
            <Input value={name} disabled={disabled} maxLength={80} onChange={(event) => setName(event.target.value)} />
          </label>
        </div>
      )}
      {backedByOpenCode && !opencodeReady ? (
        <p className="m-0 text-sm text-subtext">{m.settings_needs_opencode()}</p>
      ) : !modelsReady ? (
        <p role="status" className="m-0 flex items-center gap-2 text-sm text-subtext"><Spinner />{m.add_agent_models_loading()}</p>
      ) : listed.length > 0 ? (
        <ModelList models={listed} harness={methodHarness?.id} owner={saved?.kind === "provider" ? saved.name : undefined} />
      ) : (
        <p role="status" className="m-0 text-sm text-accent-amber">
          {saved?.kind === "endpoint" ? m.local_models_config_restricted() : backedByOpenCode ? m.add_agent_models_none() : m.settings_no_models()}{" "}
          <Button variant="ghost" size="small" onClick={() => { setChecking(true); void refreshHarness(methodHarness?.id ?? "opencode").catch(() => {}).finally(() => setChecking(false)); }}>
            <RefreshCw size={14} />{m.onboarding_re_check()}
          </Button>
        </p>
      )}
      {tile.id === "opencode-go" && uiState.data?.opencodeFreeModels && (
        <div className="flex items-center gap-3">
          <div className="min-w-0 flex-1">
            <div className="text-sm">{m.add_agent_hide_free_models()}</div>
            <p className="m-0 text-sm text-subtext">{m.add_agent_hide_free_models_detail()}</p>
          </div>
          <Switch type="button" checked={hideFreeModels} aria-label={m.add_agent_hide_free_models()} disabled={disabled} onClick={() => setHideFreeModels(!hideFreeModels)} />
        </div>
      )}
      {backedByOpenCode && opencodeReady && <p className="m-0 text-xs text-subtext">{m.settings_runs_on_opencode({ version: opencodeVersion(opencode) ?? "2" })}</p>}
    </div>;
    const label = saved ? name.trim() : tile.name;
    primary = <Button variant="primary" disabled={!modelsReady || !label || disabled} onClick={() => void finish()}>
      {busy && <Spinner />}{m.add_agent_finish({ name: label })}
    </Button>;
  }

  const index = STEPS.indexOf(step);
  const canGoBack = index > 0;

  return (
    // Its height follows the step, up to the window's.
    <DialogContent
      initialFocus={() => bodyRef.current?.querySelector<HTMLElement>("input:not([type=hidden]), [data-autofocus]") ?? bodyRef.current}
      className="flex max-h-[calc(100vh_-_40px)] w-180 flex-col p-0 text-text"
    >
      <div className="shrink-0 border-b border-border px-5 pt-5 pb-4">
        <DialogTitle className="m-0 flex items-center gap-2.5 text-xl font-medium">
          {step === "signin" && logo(22)}
          {title}
        </DialogTitle>
        {step !== "provider" && <StepBar labels={stepLabels} current={index} disabled={disabled} onStep={(target) => leave(STEPS[target])} />}
      </div>
      <div ref={bodyRef} tabIndex={-1} className="min-h-0 flex-1 overflow-y-auto px-5 py-4 outline-none">
        {body}
        {error && (
          <p role="alert" className="mt-4 mb-0 flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-accent-red first:mt-0">
            {renderNote(error)}
            {retry && <Button size="small" onClick={retry} disabled={disabled}>{m.app_retry()}</Button>}
          </p>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-2 border-t border-border px-5 py-3">
        {canGoBack && <Button variant="ghost" onClick={back} disabled={disabled}><ArrowLeft size={14} />{m.onboarding_back()}</Button>}
        <div className="ms-auto flex gap-2">
          <Button onClick={() => leave("close")} disabled={disabled}>{m.settings_page_cancel()}</Button>
          {primary}
        </div>
      </div>
    </DialogContent>
  );
}

/** The hosts each workspace has warned about, so the page and every wizard it opens warn once between them. */
const warnedHosts = new WeakMap<QueryClient, Set<string>>();

/** Runs a Claude Code sign-in on remote machine `host` only after warning, once, that a subscription signed in from a datacenter IP may be flagged. */
export function useDatacenterWarning(host: string | undefined) {
  const { queryClient, queries, dialogs } = useWorkspace();
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  return async (proceed: () => unknown, back: () => void) => {
    const warned = warnedHosts.get(queryClient) ?? new Set<string>();
    warnedHosts.set(queryClient, warned);
    if (!host || warned.has(host)) return proceed();
    const network = await queryClient.fetchQuery(queries.getPublicNetworkQuery()).catch(() => null);
    if (!mounted.current) return;
    warned.add(host);
    if (!network?.isDatacenter) return proceed();
    dialogs.showCustomDialog({ component: ClaudeDatacenterWarning, props: { host, ip: network.ip }, onConfirm: async () => { await proceed(); }, onCancel: back });
  };
}

function ClaudeDatacenterWarning({ host, ip, confirm, cancel }: DialogContentProps<void> & { host: string; ip: string }) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  return <DialogContent role="alertdialog" initialFocus={cancelRef} className="w-140 max-w-full p-6">
    <TriangleAlert size={36} className="mb-4 text-accent-amber" aria-hidden="true" />
    <DialogTitle className="mb-3 text-2xl text-accent-amber">{m.remote_claude_datacenter_title()}</DialogTitle>
    <p className="mb-5 text-base text-text">{m.remote_claude_datacenter_description({ host: ltr(host), ip: ltr(ip) })}</p>
    <div className="flex justify-end gap-2">
      <Button ref={cancelRef} variant="primary" onClick={cancel}>{m.common_cancel()}</Button>
      <Button onClick={() => confirm()}>{m.remote_claude_datacenter_continue()}</Button>
    </div>
  </DialogContent>;
}

/** Earlier steps are links back; the current and later ones are not. */
function StepBar({ labels, current, disabled, onStep }: { labels: string[]; current: number; disabled: boolean; onStep: (index: number) => void }) {
  return (
    <ol className="m-0 mt-4 grid list-none grid-cols-3 gap-1 rounded-xl border border-border bg-surface p-1">
      {labels.map((label, index) => {
        const done = index < current;
        return (
          <li key={index}>
            <button
              type="button"
              disabled={!done || disabled}
              aria-current={index === current ? "step" : undefined}
              onClick={() => onStep(index)}
              className={cn(
                "flex w-full items-center gap-2 rounded-lg border px-3 py-1.5 text-start text-sm outline-none focus-visible:outline-2 focus-visible:outline-focus disabled:cursor-default",
                index === current ? "border-border bg-background text-text" : "border-transparent",
                done ? "text-text [&:hover:not(:disabled)]:bg-hover-subtle" : index > current && "text-subtext",
              )}
            >
              <span className={cn(
                "grid size-5 shrink-0 place-items-center rounded-full text-xs font-medium",
                done ? "bg-primary text-background" : index === current ? "border border-primary text-primary" : "border border-border",
              )}>
                {done ? <Check size={12} strokeWidth={3} aria-hidden="true" /> : index + 1}
              </span>
              {label}
            </button>
          </li>
        );
      })}
    </ol>
  );
}

function SignedIn() {
  return <p role="status" className="m-0 flex items-center gap-2 text-sm"><CircleCheck size={16} className="text-accent-green" />{m.add_agent_signed_in()}</p>;
}

function SearchField({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  return (
    <label className="relative mb-3 block">
      <span className="sr-only">{label}</span>
      <Search size={14} className="pointer-events-none absolute start-2.5 top-1/2 -translate-y-1/2 text-muted" aria-hidden="true" />
      <Input
        type="search"
        autoComplete="off"
        spellCheck={false}
        placeholder={label}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        className="ps-8 [&::-webkit-search-cancel-button]:hidden"
      />
    </label>
  );
}

/** The ways one tile or provider signs in. */
function MethodList({ methods, disabled }: { methods: { label: string; detail: string; mark: ReactNode; select: () => void }[]; disabled: boolean }) {
  return <div className="grid gap-2">
    {methods.map((method, index) => (
      <button
        key={index}
        type="button"
        data-autofocus={index === 0 || undefined}
        disabled={disabled}
        onClick={method.select}
        className="flex items-center gap-3.5 rounded-xl border border-border bg-background px-4 py-3.5 text-start text-text outline-none hover:bg-surface focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2 disabled:cursor-default disabled:opacity-50 [&>svg]:shrink-0 [&>svg]:text-subtext"
      >
        {method.mark}
        <span className="grid">
          <span className="text-sm font-medium">{method.label}</span>
          {method.detail && <span className="text-xs text-subtext">{method.detail}</span>}
        </span>
      </button>
    ))}
  </div>;
}

/** The questions shown for `answers`: each field only while its every `when` holds. */
function askedFields(fields: OpenCodeFormField[], answers: Record<string, OpenCodeAnswer>) {
  return fields.filter((field) => (field.when ?? []).every((rule) => {
    const answer = answers[rule.key];
    return answer !== undefined && (rule.op === "eq") === (answer === rule.value);
  }));
}

/** OpenCode's questions before a sign-in: a choice per option list, else a text or number field. */
function SignInQuestions({ formId, fields, answers, disabled, onAnswer, onSubmit }: {
  formId: string;
  fields: OpenCodeFormField[];
  answers: Record<string, OpenCodeAnswer>;
  disabled: boolean;
  onAnswer: (key: string, value: OpenCodeAnswer) => void;
  onSubmit: () => void;
}) {
  return <form id={formId} className="grid gap-5" onSubmit={(event) => { event.preventDefault(); onSubmit(); }}>
    {fields.map((field) => {
      const title = field.title ?? field.key;
      if (field.options?.length) {
        return <div key={field.key} role="radiogroup" aria-label={title} className="grid gap-2">
          <span className="text-sm text-subtext">{title}</span>
          {field.options.map((option, index) => (
            <button
              key={option.value}
              type="button"
              role="radio"
              aria-checked={answers[field.key] === option.value}
              data-autofocus={index === 0 || undefined}
              disabled={disabled}
              onClick={() => onAnswer(field.key, option.value)}
              className="flex items-center gap-3 rounded-xl border border-border bg-background px-4 py-3 text-start text-text outline-none hover:bg-surface focus-visible:outline-2 focus-visible:outline-focus aria-checked:border-primary aria-checked:bg-surface disabled:opacity-50"
            >
              <span aria-hidden="true" className="grid size-4 shrink-0 place-items-center rounded-full border border-border">
                {answers[field.key] === option.value && <span className="size-2 rounded-full bg-primary" />}
              </span>
              <span className="grid">
                <span className="text-sm font-medium">{option.label}</span>
                {option.description && <span className="text-xs text-subtext">{option.description}</span>}
              </span>
            </button>
          ))}
        </div>;
      }
      const numeric = field.type === "number" || field.type === "integer";
      return <label key={field.key} className="grid gap-1 text-sm text-subtext">
        <span>{title}</span>
        <Input
          type={numeric ? "number" : "text"}
          autoComplete="off"
          spellCheck={false}
          placeholder={field.placeholder}
          value={String(answers[field.key] ?? "")}
          disabled={disabled}
          onChange={(event) => onAnswer(field.key, numeric ? Number(event.target.value) : event.target.value)}
        />
        {field.description && <span className="text-xs">{field.description}</span>}
      </label>;
    })}
  </form>;
}

/** A provider's sign-in through OpenCode: its page, and its code when the page shows one. */
function OAuthPanel({ signIn, disabled, onComplete, onRetry }: {
  signIn: OpenCodeSignIn;
  disabled: boolean;
  onComplete: () => void;
  onRetry: () => void;
}) {
  const { getOpenCodeSignIn, sendOpenCodeSignInCode } = useWorkspace().api;
  const codeId = useId();
  const [status, setStatus] = useState<{ status: "pending" | "complete" | "failed" | "expired"; message?: string }>({ status: "pending" });
  const [code, setCode] = useState("");
  const [sending, setSending] = useState(false);
  const [sent, setSent] = useState(false);
  /** Status checks failed in a row, each waiting twice as long as the last. */
  const [misses, setMisses] = useState(0);
  const completeRef = useRef(onComplete);
  completeRef.current = onComplete;

  useEffect(() => {
    if (status.status !== "pending") return;
    const controller = new AbortController();
    const timer = setTimeout(() => {
      void getOpenCodeSignIn(signIn.attempt, controller.signal).then((next) => {
        setMisses(0);
        setStatus(next);
        if (next.status === "complete") completeRef.current();
      }, (cause: unknown) => {
        if (controller.signal.aborted) return;
        if (misses < 4) setMisses(misses + 1);
        else setStatus({ status: "failed", message: cause instanceof Error ? cause.message : String(cause) });
      });
    }, 1500 * 2 ** misses);
    return () => { clearTimeout(timer); controller.abort(); };
  }, [signIn.attempt, status, misses]);

  const send = async () => {
    if (!code.trim() || sending) return;
    setSending(true);
    try {
      await sendOpenCodeSignInCode(signIn.attempt, code.trim());
      setSent(true);
    } catch (cause) {
      setStatus({ status: "failed", message: cause instanceof Error ? cause.message : String(cause) });
    } finally { setSending(false); }
  };

  const ended = status.status === "failed" || status.status === "expired";
  // A device sign-in's code sits in OpenCode's instructions, as in "Enter code: DEBC-F0E3".
  const deviceCode = signIn.mode === "auto" ? /\bcode:?\s+([A-Z0-9]{3,}(?:-[A-Z0-9]{3,})+)\b/i.exec(signIn.instructions)?.[1] : undefined;
  return <div className="flex flex-col gap-3">
    <SignInSteps url={signIn.url} message={deviceCode ? m.add_agent_oauth_device_steps : signIn.mode === "code" ? m.harness_setup_signin_steps_code : m.harness_setup_signin_steps} />
    {deviceCode ? (
      <div className="flex items-center gap-2">
        <code className="flex h-8 items-center rounded-md border border-border bg-surface px-3 font-mono text-lg tracking-widest select-all">{deviceCode}</code>
        <Button data-autofocus onClick={() => void copyToClipboard(deviceCode)}><Copy size={14} aria-hidden="true" />{m.md_copy()}</Button>
      </div>
    ) : signIn.mode === "auto" && signIn.instructions && <p className="m-0 text-sm text-subtext select-text wrap-anywhere">{renderNote(signIn.instructions)}</p>}
    {signIn.mode === "code" && !ended && (
      <form className="grid gap-1.5" onSubmit={(event) => { event.preventDefault(); void send(); }}>
        <label htmlFor={codeId} className="text-sm text-subtext">{m.harness_setup_signin_code()}</label>
        <div className="flex gap-2">
          <Input id={codeId} autoComplete="off" spellCheck={false} className="font-mono" value={code} disabled={disabled} onChange={(event) => { setCode(event.target.value); setSent(false); }} />
          <Button type="submit" disabled={!code.trim() || sending || sent || disabled}>{sending && <Spinner />}{m.chat_panel_send()}</Button>
        </div>
      </form>
    )}
    {ended ? (
      <p role="alert" className="m-0 flex flex-wrap items-center gap-2 text-sm text-accent-red">
        {status.message ?? (status.status === "expired" ? m.add_agent_oauth_expired() : m.add_agent_oauth_failed())}
        <Button size="small" onClick={onRetry} disabled={disabled}>{m.app_retry()}</Button>
      </p>
    ) : (
      <p role="status" className="m-0 flex min-h-5 items-center gap-2 text-sm text-subtext">
        <Spinner />{sent ? m.harness_setup_signin_checking_code() : m.harness_setup_signin_waiting()}
      </p>
    )}
  </div>;
}

/** Signs a saved provider in again through OpenCode; `start` begins each attempt. */
export function OpenCodeSignInDialog({ name, first, start, confirm, cancel, isDisabled }: DialogContentProps<void> & {
  name: string;
  first: OpenCodeSignIn;
  start: () => Promise<OpenCodeSignIn>;
}) {
  const { cancelOpenCodeSignIn } = useWorkspace().api;
  const [signIn, setSignIn] = useState(first);
  const [error, setError] = useState<string | null>(null);
  return <DialogContent className="w-140 p-5 text-text">
    <DialogTitle className="m-0 mb-4 text-xl font-medium">{m.add_agent_sign_in_title({ name })}</DialogTitle>
    <OAuthPanel
      key={signIn.attempt}
      signIn={signIn}
      disabled={isDisabled}
      onComplete={() => confirm()}
      onRetry={() => { setError(null); start().then(setSignIn, (cause: unknown) => setError(cause instanceof Error ? cause.message : String(cause))); }}
    />
    {error && <p role="alert" className="mt-3 mb-0 text-sm text-accent-red">{error}</p>}
    <div className="mt-5 flex justify-end">
      <Button onClick={() => { void cancelOpenCodeSignIn(signIn.attempt).catch(() => {}); cancel(); }} disabled={isDisabled}>{m.settings_page_cancel()}</Button>
    </div>
  </DialogContent>;
}

/** OpenCode 1 can't add providers: update the user's own, or use OpenResearch's copy. */
function OpenCodeUpdate({ opencode, commands, disabled, onReady }: {
  opencode: Harness;
  commands: HarnessSetupCommands;
  disabled: boolean;
  onReady: () => void;
}) {
  const [action, setAction] = useState<"upgrade" | "install" | null>(null);
  if (action) return <HarnessSetup key={action} harness={opencode} commands={commands} action={action} disabled={disabled} onReady={onReady} />;
  return <form className="space-y-3" onSubmit={(event) => { event.preventDefault(); setAction("upgrade"); }}>
    <p className="m-0 text-sm font-medium">{m.opencode_update_title()}</p>
    <p className="m-0 text-sm text-subtext">{m.opencode_update_description({ version: opencodeVersion(opencode) ?? "1" })}</p>
    <div className="flex flex-wrap gap-2">
      <Button type="submit" variant="primary" data-autofocus disabled={disabled}>{m.opencode_update()}</Button>
      <Button type="button" disabled={disabled} onClick={() => setAction("install")}>{m.opencode_use_copy()}</Button>
    </div>
  </form>;
}

/** An agent's own setup: install, then its sign-in; or what blocks it here. */
function AgentSignIn({ harness, commands, remote, disabled, definition, formId, onReady, onEdit }: {
  harness: Harness;
  commands?: HarnessSetupCommands;
  remote: boolean;
  disabled: boolean;
  definition?: AcpDefinition;
  formId?: string;
  onReady: () => void;
  onEdit: (definition: AcpDefinition) => void;
}) {
  const { refreshHarnesses } = useWorkspace().queries;
  const [auth, setAuth] = useState<Harness | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  if (auth || (harness.id.startsWith("acp:") && harness.installed && !harness.adapterMissing)) {
    return <AcpAuth harness={auth ?? harness} remote={remote} disabled={disabled} formId={formId} onReady={onReady} />;
  }
  if (canConfigureHarness(harness, remote, commands) && commands) {
    return <HarnessSetup key="setup" harness={harness} commands={commands} disabled={disabled} onReady={onReady} onAuth={setAuth} />;
  }
  const needsZCode = harness.logo === "zcode-acp-server" && !harness.installed && !harness.adapterMissing;
  return <div className="space-y-4">
    <p className="m-0 text-sm text-subtext" role="status">
      {needsZCode ? m.harness_zcode_required() : renderNote(harness.agentNote) ?? m.harness_install_help({ name: harness.name })}
    </p>
    <div className="flex flex-wrap items-center gap-2">
      <Button onClick={() => { setRefreshing(true); void refreshHarnesses(true, true).catch(() => {}).finally(() => setRefreshing(false)); }} disabled={refreshing}>
        <RefreshCw size={14} />{m.settings_page_refresh()}
      </Button>
      {harness.setupUrl && (
        <a href={needsZCode ? "https://zcode.z.ai/en/docs/install" : harness.setupUrl} target="_blank" rel="noreferrer" className="text-sm text-accent-blue-strong underline">{m.acp_setup_docs()}</a>
      )}
      {definition && !harness.setupUrl && <Button onClick={() => onEdit(definition)}>{m.activity_edit()}</Button>}
    </div>
  </div>;
}

/** `owner` is the record's saved name, which a second key's models carry after their own. */
function ModelList({ models, harness, owner }: { models: HarnessModel[]; harness?: HarnessId; owner?: string }) {
  const own = (model: HarnessModel) => owner && model.displayName?.endsWith(` · ${owner}`) ? { ...model, displayName: model.displayName.slice(0, -` · ${owner}`.length) } : model;
  const lines = agentModelLines(harness ?? "opencode", models.map(own));
  if (lines.length <= 1) {
    const model = lines[0].offers[0].model;
    return <p className="m-0 flex items-center gap-2 text-sm wrap-anywhere"><ModelMark model={model} harness={harness} size={16} />{models.length === 1 ? harnessModelLabel(model) : tidyModelName(lines[0].label)}</p>;
  }
  const shown = lines.slice(0, 10);
  return <div>
    <p className="m-0 mb-2 flex items-center gap-2 text-sm"><CircleCheck size={16} className="text-accent-green" />{m.settings_models_available({ count: fmtNumber(lines.length) })}</p>
    <ul className="m-0 grid list-none grid-cols-2 gap-x-6 gap-y-1.5 rounded-lg border border-border bg-surface px-4 py-3 text-sm text-subtext">
      {shown.map((line) => <li key={line.key} className="flex min-w-0 items-center gap-2 wrap-anywhere"><ModelMark model={line.offers[0].model} harness={harness} size={14} />{tidyModelName(line.label)}</li>)}
    </ul>
    {lines.length > shown.length && <p className="m-0 mt-2 text-xs text-subtext">{m.add_agent_models_more({ count: fmtNumber(lines.length - shown.length) })}</p>}
  </div>;
}

function DiscardDialog({ trigger, confirm, cancel, isDisabled, isPending, errorMessage }: DialogContentProps<void> & { trigger: "back" | "close" }) {
  const keepRef = useRef<HTMLButtonElement>(null);
  return <DialogContent initialFocus={keepRef}>
    <DialogHeader>
      <DialogTitle>{trigger === "back" ? m.add_agent_discard_back_title() : m.add_agent_discard_close_title()}</DialogTitle>
      <DialogDescription>{m.add_agent_discard_description()}</DialogDescription>
    </DialogHeader>
    {errorMessage && <p role="alert" className="mt-3 mb-0 text-sm text-accent-red">{errorMessage}</p>}
    <DialogFooter>
      <Button variant="danger" onClick={() => confirm()} disabled={isDisabled}>
        {isPending && <Spinner />}{trigger === "back" ? m.add_agent_discard_back() : m.add_agent_discard_close()}
      </Button>
      <Button ref={keepRef} variant="primary" onClick={cancel} disabled={isDisabled}>{m.add_agent_continue_setup()}</Button>
    </DialogFooter>
  </DialogContent>;
}

type Choice = {
  key: string;
  name: string;
  blurb?: string;
  mark: ReactNode;
  unavailable?: boolean;
  select: () => void;
};

export function ChoiceGrid({ choices, columns, featured = false }: { choices: Choice[]; columns: 2 | 3 | 4; featured?: boolean }) {
  return (
    <div className={cn("grid gap-2", columns === 4 ? "grid-cols-2 sm:grid-cols-4" : columns === 3 ? "grid-cols-1 sm:grid-cols-3" : "grid-cols-1 sm:grid-cols-2")}>
      {choices.map((choice) => (
        <button
          key={choice.key}
          type="button"
          aria-disabled={choice.unavailable || undefined}
          onClick={() => { if (!choice.unavailable) choice.select(); }}
          className={cn(
            "flex w-full items-center gap-3 rounded-lg border border-border bg-background p-3 text-start text-sm outline-none hover:bg-surface focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2 aria-disabled:cursor-not-allowed aria-disabled:opacity-50",
            featured ? "min-h-16" : "min-h-12",
          )}
        >
          {choice.mark}
          <span className="flex min-w-0 flex-col">
            <span className="font-medium">{choice.name}</span>
            {choice.blurb && <span className="text-xs text-subtext">{choice.blurb}</span>}
          </span>
        </button>
      ))}
    </div>
  );
}
