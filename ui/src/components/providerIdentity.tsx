import { Bot, Cloud, CodeXml, FlaskConical, Laptop, Route, Server, type LucideIcon } from "lucide-react";
import type { AgentIcon, AgentIconColor, FoundProvider } from "../api";
import { BRAND_ICON, type Logo } from "./AgentIcon";
import { labNamed, LOGOS, logoMark, squashName } from "./HarnessLogo";
import { EndpointMark, endpointServer } from "./LocalModelSetup";

/** Names that point at a logo no lab pattern knows them by. */
const ALIASES: [pattern: RegExp, slug: string][] = [
  [/tinker|thinking-?machines/, "thinkingmachines"],
  [/azure|foundry/, "microsoft"],
  [/vertex/, "google"],
  [/ai-?studio/, "gemini"],
  [/bedrock/, "bedrock"],
  [/\baws\b|amazon/, "amazon"],
  [/copilot/, "github-copilot"],
  [/hugging-?face|\bhf\b/, "huggingface"],
  [/together/, "together"],
  [/fireworks/, "fireworks"],
  [/cerebras/, "cerebras"],
  [/\bgroq\b/, "groq"],
  [/deepinfra/, "deepinfra"],
  [/baseten/, "baseten"],
  [/vercel/, "vercel"],
  [/ollama/, "ollama"],
  [/dashscope|alibaba|bailian|aliyun/, "qwen"],
  [/bigmodel/, "zai"],
  [/volc|\bark\b/, "doubao"],
  [/anthropic/, "anthropic"],
];

/** The logo a name, id, slug or key variable points at: `thinking-machines`, `TINKER_API_KEY`, `Azure work`. */
export function guessLogo(text: string): string | undefined {
  const words = text.toLowerCase().replace(/_api_key$|_key$|_token$/, "").replace(/[_\s]+/g, "-");
  const squashed = squashName(words);
  if (!squashed) return undefined;
  const exact = LOGOS.find((logo) => logo.slug === words || squashName(logo.slug) === squashed || squashName(logo.name) === squashed);
  if (exact) return exact.slug;
  const alias = ALIASES.find(([pattern]) => pattern.test(words))?.[1];
  if (alias) return alias;
  const lab = labNamed(words);
  return lab === "claude" ? "claude" : lab;
}

/** API hosts by the logo of whoever runs them. */
const HOSTS: [pattern: RegExp, slug: string][] = [
  [/(^|\.)openai\.com$/, "openai"],
  [/(^|\.)azure\.com$|(^|\.)microsoft\.com$/, "microsoft"],
  [/(^|\.)anthropic\.com$/, "anthropic"],
  [/^generativelanguage\.googleapis\.com$/, "gemini"],
  [/(^|\.)googleapis\.com$/, "google"],
  [/(^|\.)openrouter\.ai$/, "openrouter"],
  [/(^|\.)deepseek\.com$/, "deepseek"],
  [/(^|\.)moonshot\.(ai|cn)$/, "moonshot"],
  [/(^|\.)kimi\.(com|ai)$/, "kimi"],
  [/(^|\.)z\.ai$|(^|\.)bigmodel\.cn$/, "zai"],
  [/(^|\.)x\.ai$/, "grok"],
  [/(^|\.)groq\.com$/, "groq"],
  [/(^|\.)cerebras\.ai$/, "cerebras"],
  [/(^|\.)together\.(xyz|ai)$/, "together"],
  [/(^|\.)fireworks\.ai$/, "fireworks"],
  [/(^|\.)mistral\.ai$/, "mistral"],
  [/(^|\.)nvidia\.com$/, "nvidia"],
  [/(^|\.)huggingface\.co$|(^|\.)hf\.co$/, "huggingface"],
  [/(^|\.)deepinfra\.com$/, "deepinfra"],
  [/(^|\.)baseten\.co$/, "baseten"],
  [/^bedrock.*\.amazonaws\.com$/, "bedrock"],
  [/(^|\.)amazonaws\.com$/, "amazon"],
  [/(^|\.)perplexity\.ai$/, "perplexity"],
  [/(^|\.)cohere\.(ai|com)$/, "cohere"],
  [/(^|\.)vercel\.(sh|com|app)$/, "vercel"],
  [/(^|\.)ollama\.com$/, "ollama"],
  [/(^|\.)aliyuncs\.com$|(^|\.)alibabacloud\.com$|(^|\.)qwen\.ai$/, "qwen"],
  [/(^|\.)minimaxi?\.(io|com|chat)$/, "minimax"],
  [/(^|\.)thinkingmachines\.(ai|dev)$/, "thinkingmachines"],
  [/(^|\.)ai21\.com$/, "ai21"],
  [/(^|\.)reka\.ai$/, "reka"],
  [/(^|\.)inceptionlabs\.ai$/, "inception"],
  [/(^|\.)githubcopilot\.com$|(^|\.)github\.ai$/, "github-copilot"],
  [/(^|\.)upstage\.ai$/, "upstage"],
  [/(^|\.)writer\.com$/, "writer"],
  [/(^|\.)inference\.net$/, "inference"],
  [/(^|\.)morphllm\.com$/, "morph"],
  [/(^|\.)relace\.run$/, "relace"],
  [/(^|\.)xiaomimimo\.com$|(^|\.)xiaomi\.com$/, "xiaomi"],
  [/(^|\.)volces\.com$|(^|\.)volcengine\.com$|(^|\.)byteplus(api)?\.com$/, "doubao"],
  [/(^|\.)stepfun\.com$/, "stepfun"],
  [/(^|\.)lingyiwanwu\.com$|(^|\.)01\.ai$/, "yi"],
  [/(^|\.)baichuan-ai\.com$/, "baichuan"],
  [/(^|\.)baidubce\.com$/, "baidu"],
  [/(^|\.)tencentcloudapi\.com$|(^|\.)hunyuan\.cloud\.tencent\.com$/, "hunyuan"],
  [/(^|\.)sensenova\.cn$/, "sensenova"],
  [/(^|\.)liquid\.ai$/, "liquid"],
  [/(^|\.)nousresearch\.com$/, "nousresearch"],
  [/(^|\.)arcee\.ai$/, "arcee"],
  [/(^|\.)poolside\.ai$/, "poolside"],
  [/(^|\.)ibm\.com$/, "ibm"],
  [/(^|\.)databricks\.com$|(^|\.)azuredatabricks\.net$/, "dbrx"],
  [/(^|\.)llama\.com$|(^|\.)meta\.ai$/, "meta"],
  [/(^|\.)sakana\.ai$/, "sakana"],
  [/(^|\.)cursor\.(sh|com)$/, "cursor"],
  [/(^|\.)opencode\.ai$/, "opencode"],
];

/** AI SDK packages tied to one vendor; the protocol packages others reuse count only without an address. */
const PACKAGES: [pattern: RegExp, slug: string][] = [
  [/^@ai-sdk\/anthropic$/, "anthropic"],
  [/^@ai-sdk\/openai$/, "openai"],
  [/^@ai-sdk\/azure$/, "microsoft"],
  [/^@ai-sdk\/google-vertex/, "google"],
  [/^@ai-sdk\/google$/, "gemini"],
  [/^@ai-sdk\/amazon-bedrock$/, "bedrock"],
  [/^@ai-sdk\/mistral$/, "mistral"],
  [/^@ai-sdk\/groq$/, "groq"],
  [/^@ai-sdk\/xai$/, "grok"],
  [/^@ai-sdk\/deepseek$/, "deepseek"],
  [/^@ai-sdk\/cerebras$/, "cerebras"],
  [/^@ai-sdk\/togetherai$/, "together"],
  [/^@ai-sdk\/fireworks$/, "fireworks"],
  [/^@ai-sdk\/perplexity$/, "perplexity"],
  [/^@ai-sdk\/cohere$/, "cohere"],
  [/^@ai-sdk\/deepinfra$/, "deepinfra"],
  [/^@ai-sdk\/(vercel|gateway)$/, "vercel"],
  [/^@openrouter\//, "openrouter"],
  [/ollama/, "ollama"],
];

function host(url: string | undefined) {
  if (!url) return undefined;
  try { return new URL(url).hostname.toLowerCase(); } catch { return undefined; }
}

const isLoopback = (hostname: string) => hostname === "localhost" || hostname === "::1" || hostname === "[::1]" || hostname.startsWith("127.");

/** The logo an endpoint's address points at. */
export function hostLogo(url: string | undefined): string | undefined {
  const hostname = host(url);
  return hostname ? HOSTS.find(([pattern]) => pattern.test(hostname))?.[1] : undefined;
}

/** Glyphs a provider no logo knows wears, by what its name or address suggests. */
const GLYPHS: [pattern: RegExp, Glyph: LucideIcon][] = [
  [/local|localhost|127\.0\.0\.1|\blan\b|home/, Laptop],
  [/rout|gateway|proxy|relay|\bhub\b|llmtr|litellm/, Route],
  [/gpu|cluster|compute|server|vllm|sglang|\btgi\b|inference/, Server],
  [/cloud/, Cloud],
  [/\blab\b|research|univ|\.edu|science/, FlaskConical],
  [/code|coder|\bdev\b/, CodeXml],
];
const GENERATED_COLORS: AgentIconColor[] = ["blue", "teal", "green", "amber", "orange", "red", "pink", "purple", "indigo"];

/** A glyph and colour for a provider no logo knows, the same each time for the same provider. */
function generatedMark(id: string, hints: string): { brand: Logo; color: AgentIconColor } {
  const Glyph = GLYPHS.find(([pattern]) => pattern.test(hints))?.[1] ?? Bot;
  const hash = [...id].reduce((sum, char) => (sum * 31 + char.charCodeAt(0)) >>> 0, 7);
  return { brand: (size) => <Glyph size={size} aria-hidden="true" />, color: GENERATED_COLORS[hash % GENERATED_COLORS.length] };
}

/** Whether `short` spells `long` in order from its first letter, as `or` does OpenRouter and `tinker` doesn't Thinking Machines. */
function abbreviates(short: string, long: string) {
  let at = 0;
  for (const char of short) {
    at = long.indexOf(char, at) + 1;
    if (at === 0) return false;
  }
  return short[0] === long[0];
}

/** `my-router` as a name: `My Router`. */
function titleCase(id: string) {
  return id.split(/[-_\s]+/).filter(Boolean).map((word) => word[0].toUpperCase() + word.slice(1)).join(" ");
}

/** How a provider found in the user's OpenCode presents: its name, logo, and the icon it wears until the user picks one. */
export function foundProviderLook(provider: FoundProvider, known?: { name: string; logo: Logo }) {
  const label = provider.label ?? {};
  const pinned = logoMark(label.logo);
  const hostname = host(provider.baseUrl);
  const server = hostname && isLoopback(hostname) ? endpointServer({ name: provider.name ?? "", baseUrl: provider.baseUrl! }) : undefined;
  const slug = guessLogo(provider.id)
    ?? hostLogo(provider.baseUrl)
    ?? (provider.baseUrl ? undefined : PACKAGES.find(([pattern]) => pattern.test(provider.npm ?? ""))?.[1])
    ?? provider.env?.map(guessLogo).find(Boolean)
    ?? (provider.name ? guessLogo(provider.name) : undefined);
  const guessed = logoMark(slug);
  const generated = generatedMark(provider.id, `${provider.id} ${provider.name ?? ""} ${provider.baseUrl ?? ""}`.toLowerCase());

  // An id that spells the logo's own name reads as that name; any other id is the user's own word for it.
  const logoName = LOGOS.find((logo) => logo.slug === slug)?.name;
  const ownName = logoName && abbreviates(squashName(provider.id), squashName(logoName)) ? logoName : titleCase(provider.id);
  const name = label.name ?? provider.name ?? known?.name ?? ownName;

  const brand: Logo = pinned ?? known?.logo ?? guessed ?? (server?.icon ? (size) => <EndpointMark server={server} size={size} /> : generated.brand);
  const plain = Boolean(pinned ?? known ?? guessed ?? server?.icon);
  const icon: AgentIcon = label.icon ?? (plain ? BRAND_ICON : { brand: true, color: generated.color });
  return { name, icon, brand };
}
