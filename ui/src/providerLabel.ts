/** Display names for the providers in OpenCode's catalog, whose ids are the
 * provider's own handle rather than a label. Anything not listed keeps its id:
 * OpenCode ships more providers than we can enumerate, and inventing a prettier
 * spelling for an unknown one would be wrong more often than right. */
const OPENCODE_PROVIDER_LABELS: Record<string, string> = {
  opencode: "OpenCode",
  "opencode-go": "OpenCode Go",
  openrouter: "OpenRouter",
  anthropic: "Anthropic",
  openai: "OpenAI",
  google: "Google",
  "github-copilot": "GitHub Copilot",
  cerebras: "Cerebras",
  deepseek: "DeepSeek",
  xai: "xAI",
  mistral: "Mistral",
  groq: "Groq",
};

/** `openrouter/moonshotai/kimi-k3.1-pro` → `OpenRouter`. `null` when the id
 * carries no provider prefix. */
export function opencodeProviderLabel(id: string): string | null {
  const index = id.indexOf("/");
  if (index <= 0) return null;
  const provider = id.slice(0, index);
  return OPENCODE_PROVIDER_LABELS[provider] ?? provider;
}

/** Append the provider a model comes from to its label.
 *
 * OpenCode's catalog is every provider the user connected *to* OpenCode, so the
 * same model name can appear twice (OpenCode Go and OpenRouter both serve
 * `kimi-k3.1-pro`) with different prices, limits, and routing. Naming the
 * provider keeps those rows distinguishable; without it the picker shows two
 * identical entries and no way to tell which one a chat will call.
 *
 * Scoped to the OpenCode harness: Claude, Codex and Cursor ids carry no provider
 * prefix, and an ACP catalog's ids are the agent's own — appending a segment we
 * guessed at would make those labels worse, not better.
 */
export function withProviderSuffix(
  label: string,
  id: string,
  harness?: string | null,
): string {
  const provider = harness === "opencode" ? opencodeProviderLabel(id) : null;
  // A catalog name that already says it ("Big Pickle · OpenCode") must not
  // gain a second copy of the same word.
  return provider && !label.includes(provider) ? `${label} · ${provider}` : label;
}
