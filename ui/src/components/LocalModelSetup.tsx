import { useWorkspace } from "../workspace";
import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { Link2 } from "lucide-react";
import { type LocalModelConnection } from "../api";

import { m } from "../paraglide/messages.js";
import { OptionPicker } from "./ModelPicker";
import lmStudioLogo from "../assets/lm-studio-logo.svg";
import ollamaLogo from "../assets/ollama-logo.png";
import omlxLogo from "../assets/omlx-logo.svg";
import { Input } from "./ui";

const LM_STUDIO = { id: "lmstudio", label: "LM Studio", icon: lmStudioLogo, url: "http://127.0.0.1:1234/v1" };
const SERVERS = [
  LM_STUDIO,
  { id: "omlx", label: "oMLX", icon: omlxLogo, url: "http://127.0.0.1:8000/v1" },
  { id: "ollama", label: "Ollama", icon: ollamaLogo, url: "http://127.0.0.1:11434/v1" },
  { id: "openai-compatible", label: "Custom Endpoint", icon: null, url: "http://127.0.0.1:8000/v1" },
];

type Server = (typeof SERVERS)[number];

/** The server a connection was made for: by the name it was given, else by its port. */
export function endpointServer(connection: Pick<LocalModelConnection, "name" | "baseUrl">): Server | undefined {
  const port = (url: string) => { try { return new URL(url).port; } catch { return null; } };
  return SERVERS.find((item) => item.label === connection.name || (item.id === "openai-compatible" && ["OpenAI-compatible server", "Custom endpoint"].includes(connection.name)))
    ?? SERVERS.find((item) => item.icon && port(item.url) === port(connection.baseUrl));
}

export function EndpointMark({ server, size }: { server: Server | undefined; size: number }) {
  if (!server?.icon) return <Link2 size={size} className="shrink-0" aria-hidden="true" />;
  return <img src={server.icon} alt="" width={size} height={size} className={`shrink-0 object-contain${server.id === "ollama" ? " dark:invert" : ""}`} style={{ width: size, height: size }} />;
}

type ProviderDraft = {
  baseUrl: string;
  apiKey: string;
  models: string[];
  model: string | null;
  contextWindow: string;
  error: string | null;
};

/** The custom endpoint form's state and fields; the caller owns the form element and its buttons. */
export function useLocalModelForm(disabled: boolean) {
  const { getHarnessesQuery } = useWorkspace().queries;
  const { discoverLocalModels } = useWorkspace().api;

  const harnesses = useQuery(getHarnessesQuery());
  const installed = harnesses.data?.find((harness) => harness.id === "opencode")?.installed ?? false;
  const cancelled = useRef(false);
  cancelled.current = disabled;
  useEffect(() => {
    cancelled.current = false;
    return () => { cancelled.current = true; };
  }, []);
  const [serverId, setServerId] = useState("lmstudio");
  const server = SERVERS.find((s) => s.id === serverId) ?? LM_STUDIO;
  const [drafts, setDrafts] = useState<Record<string, ProviderDraft>>({});
  const draft = drafts[server.id] ?? {
    baseUrl: server.url, apiKey: "", models: [], model: null,
    contextWindow: "32768", error: null,
  };
  const { baseUrl, models, model, contextWindow, error } = draft;
  const apiKey = server.id === "ollama" ? "" : draft.apiKey;
  const updateDraft = (changes: Partial<ProviderDraft>) => {
    setDrafts((current) => ({ ...current, [server.id]: { ...(current[server.id] ?? draft), ...changes } }));
  };
  const probe = useMutation({ mutationFn: discoverLocalModels });
  const context = Number(contextWindow);
  const busy = probe.isPending || disabled;
  const found = models.length > 0;
  const canSubmit = !busy && (!found || Boolean(model && Number.isSafeInteger(context) && context >= 4096));
  const reset = (changes: Partial<ProviderDraft> = {}) => updateDraft({ models: [], model: null, error: null, ...changes });
  const check = async () => {
    reset();
    try {
      const result = await probe.mutateAsync({ baseUrl: baseUrl.trim(), apiKey });
      if (!cancelled.current) updateDraft({ models: result.models, model: result.models[0] ?? null });
    } catch (e) { if (!cancelled.current) updateDraft({ error: e instanceof Error ? e.message : String(e) }); }
  };
  const request = model ? { name: server.label, baseUrl: baseUrl.trim(), apiKey, model, contextWindow: context } : null;

  const fields = (
    <div className="space-y-4">
      {!installed && <p className="text-sm text-subtext">{m.provider_key_installs_opencode()}</p>}
      <OptionPicker choices={SERVERS} value={serverId} title={m.local_models_server()} header={m.local_models_server()} variant="field" floating dropDown disabled={busy}
        renderLabel={(choice) => <>{choice.id === "openai-compatible" ? m.local_models_custom_endpoint() : choice.label}{choice.id === "openai-compatible" && <span className="ml-1 text-xs font-normal text-subtext">{m.local_models_compatible_api()}</span>}</>}
        renderIcon={(choice) => {
          const icon = SERVERS.find((server) => server.id === choice.id)?.icon;
          return icon ? <img src={icon} alt="" width={14} height={14} className={`size-3.5 shrink-0 object-contain${choice.id === "ollama" ? " dark:invert" : ""}`} /> : <Link2 size={14} className="shrink-0" />;
        }}
        onSelect={setServerId} />
      <label className="block space-y-1 text-sm text-subtext">
        <span>{m.local_models_address()}</span>
        <Input value={baseUrl} disabled={busy} onChange={(e) => { reset({ baseUrl: e.target.value }); }} />
      </label>
      {server.id !== "ollama" && (
        <details key={server.id} className="text-sm text-text">
          <summary className="cursor-pointer">{m.settings_page_authentication()}</summary>
          <label className="block space-y-1 mt-3 text-sm text-subtext">
            <span>{server.id === "lmstudio" ? m.local_models_token() : m.local_models_key()}</span>
            <Input type="password" autoComplete="off" value={apiKey} disabled={busy} onChange={(e) => reset({ apiKey: e.target.value })} />
          </label>
        </details>
      )}
      {found && <>
        <OptionPicker choices={models.map((id) => ({ id, label: id }))} value={model} searchPlaceholder={m.model_picker_search_models()} title={m.model_picker_model()} header={m.model_picker_model()} variant="field" floating dropDown disabled={busy} onSelect={(id) => { updateDraft({ model: id }); }} />
        <label className="block space-y-1 text-sm text-subtext">
          <span>{m.local_models_context()}</span>
          <Input type="number" min={4096} step={1024} value={contextWindow} disabled={busy} onChange={(e) => { updateDraft({ contextWindow: e.target.value }); }} />
        </label>
        {server.id !== "openai-compatible" && <p className="text-sm text-subtext">{m.local_models_context_hint()}</p>}
      </>}
    </div>
  );

  return { fields, server, installed, found, canSubmit, busy, check, request, error, setError: (message: string | null) => updateDraft({ error: message }) };
}
