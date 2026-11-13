import { useWorkspace } from "../workspace";
import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { ExternalLink, Link2, RefreshCw } from "lucide-react";
import { UnlistedError, type LocalModelConnection } from "../api";
import { fmtBytes, ltr } from "../i18n";

import { m } from "../paraglide/messages.js";
import { OptionPicker } from "./ModelPicker";
import lmStudioLogo from "../assets/lm-studio-logo.svg";
import ollamaLogo from "../assets/ollama-logo.png";
import omlxLogo from "../assets/omlx-logo.svg";
import { Button, Input, Spinner } from "./ui";

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

const CUSTOM = SERVERS[SERVERS.length - 1];

/** A model server found answering where orx runs: the models it lists, or that it wants a key first. */
export type LocalServer = { server: Server; models: string[]; needsKey: boolean };

/** Why a server listed no models; an orx from before the codes says so only in these messages. */
function unlisted(error: unknown) {
  if (error instanceof UnlistedError) return error.code;
  const message = error instanceof Error ? error.message : "";
  if (message.startsWith("The server requires an API key.")) return "needsKey";
  if (message.startsWith("The server is running but has no models.")) return "noModels";
  if (message.startsWith("Cannot reach the local server.")) return "unreachable";
  return null;
}

/** Asks each model server's usual address where orx runs; a failure other than nothing answering fails the check. */
export function useLocalServers() {
  const { api, workspaceKey } = useWorkspace();
  return useQuery({
    queryKey: workspaceKey("localServers"),
    queryFn: async () => {
      const answers = await Promise.all(SERVERS.filter((server) => server.icon).map(async (server): Promise<LocalServer | null> => {
        try {
          return { server, models: (await api.discoverLocalModels({ baseUrl: server.url, apiKey: "" })).models, needsKey: false };
        } catch (error) {
          const code = unlisted(error);
          if (code === "unreachable") return null;
          if (!code) throw error;
          return { server, models: [], needsKey: code === "needsKey" };
        }
      }));
      return answers.filter((answer) => answer !== null);
    },
    staleTime: 0,
    retry: false,
  });
}

const sameAddress = (a: string, b: string) => a.trim().replace(/\/+$/, "") === b.replace(/\/+$/, "");

/** The custom endpoint form's state and fields: an address first, then its key if the server asks for one. */
export function useLocalModelForm(disabled: boolean) {
  const { getHarnessesQuery } = useWorkspace().queries;
  const { discoverLocalModels } = useWorkspace().api;

  const harnesses = useQuery(getHarnessesQuery());
  const installed = harnesses.data?.find((harness) => harness.id === "opencode")?.installed ?? false;
  const running = useLocalServers();
  const cancelled = useRef(false);
  cancelled.current = disabled;
  useEffect(() => {
    cancelled.current = false;
    return () => { cancelled.current = true; };
  }, []);

  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [askKey, setAskKey] = useState(false);
  const [models, setModels] = useState<string[]>([]);
  const [model, setModel] = useState<string | null>(null);
  const [contextWindow, setContextWindow] = useState("32768");
  const [error, setError] = useState<string | null>(null);
  const server = running.data?.find((found) => sameAddress(baseUrl, found.server.url))?.server ?? CUSTOM;

  const list = (found: string[]) => {
    setModels(found);
    setModel(found[0] ?? null);
  };
  /** A new address starts over: its models, its key and any error belonged to the last one. */
  const address = (value: string) => {
    setBaseUrl(value);
    setApiKey("");
    setAskKey(false);
    list([]);
    setError(null);
  };
  /** Takes up a server found running, with the models it listed or its ask for a key. */
  const adopt = (found: LocalServer) => {
    address(found.server.url);
    setAskKey(found.needsKey);
    list(found.models);
  };

  const probe = useMutation({ mutationFn: discoverLocalModels });
  const context = Number(contextWindow);
  const busy = probe.isPending || disabled;
  const found = models.length > 0;
  const canSubmit = !busy && (found ? Boolean(model && Number.isSafeInteger(context) && context >= 4096) : Boolean(baseUrl.trim()) && (!askKey || Boolean(apiKey.trim())));
  const check = async () => {
    list([]);
    setError(null);
    try {
      const result = await probe.mutateAsync({ baseUrl: baseUrl.trim(), apiKey: apiKey.trim() });
      if (!cancelled.current) list(result.models);
    } catch (e) {
      if (cancelled.current) return;
      if (unlisted(e) === "needsKey" && !askKey) setAskKey(true);
      else setError(e instanceof Error ? e.message : String(e));
    }
  };
  /** Back from the key to the address; false when there's nowhere to go back to here. */
  const back = () => {
    if (!askKey) return false;
    setAskKey(false);
    setApiKey("");
    list([]);
    setError(null);
    return true;
  };
  const request = model ? { name: server.label, baseUrl: baseUrl.trim(), apiKey: apiKey.trim(), model, contextWindow: context } : null;

  const presets = running.data?.filter((answer) => !sameAddress(baseUrl, answer.server.url)) ?? [];
  const fields = (
    <div className="grid gap-4">
      {!installed && <p className="m-0 text-sm text-subtext">{m.provider_key_installs_opencode()}</p>}
      {askKey ? (
        <>
          <p className="m-0 text-sm">{m.local_models_needs_key({ address: ltr(baseUrl.trim()) })}</p>
          <label className="grid gap-1 text-sm text-subtext">
            <span>{m.add_agent_api_key()}</span>
            <Input type="password" autoComplete="off" autoFocus value={apiKey} disabled={busy} onChange={(e) => { setApiKey(e.target.value); list([]); setError(null); }} />
          </label>
        </>
      ) : (
        <>
          <label className="grid gap-1 text-sm text-subtext">
            <span>{m.local_models_address()}</span>
            <Input value={baseUrl} placeholder={CUSTOM.url} spellCheck={false} disabled={busy} onChange={(e) => address(e.target.value)} />
          </label>
          {presets.length > 0 && (
            <div className="grid gap-2 text-sm text-subtext">
              <span>{m.local_models_running()}</span>
              <div className="flex flex-wrap gap-2">
                {presets.map((answer) => (
                  <Button key={answer.server.id} size="small" disabled={busy} onClick={() => adopt(answer)}>
                    <EndpointMark server={answer.server} size={14} />{answer.server.label}
                  </Button>
                ))}
              </div>
            </div>
          )}
        </>
      )}
      {found && <>
        <OptionPicker choices={models.map((id) => ({ id, label: id }))} value={model} searchPlaceholder={m.model_picker_search_models()} title={m.model_picker_model()} header={m.model_picker_model()} variant="field" floating dropDown disabled={busy} onSelect={setModel} />
        <label className="grid gap-1 text-sm text-subtext">
          <span>{m.local_models_context()}</span>
          <Input type="number" min={4096} step={1024} value={contextWindow} disabled={busy} onChange={(e) => setContextWindow(e.target.value)} />
        </label>
        {server !== CUSTOM && <p className="m-0 text-sm text-subtext">{m.local_models_context_hint()}</p>}
      </>}
    </div>
  );

  return { fields, server, installed, found, canSubmit, busy, check, back, adopt, request, error, setError };
}

/** The runtimes the guide walks through, each where it serves by default. */
const RUNTIMES = SERVERS.filter((server) => server.id === "lmstudio" || server.id === "ollama");

/** The model size a machine's memory runs comfortably, quantized the way local runtimes ship models. */
function modelSize(memBytes: number) {
  const gb = memBytes / 2 ** 30;
  return gb < 12 ? "4B" : gb < 24 ? "8B" : gb < 48 ? "30B" : gb < 96 ? "70B" : "120B";
}

/**
 * Finds a local model server running where orx runs and hands it to `onFound`; with none running, walks through
 * installing one, picking the one already running without models, else LM Studio on macOS and Ollama elsewhere.
 */
export function LocalModelGuide({ disabled, onFound }: { disabled: boolean; onFound: (found: LocalServer) => void }) {
  const { getLocalMachineQuery } = useWorkspace().queries;
  const machine = useQuery(getLocalMachineQuery());
  const [picked, setPicked] = useState<string | null>(null);
  const detect = useLocalServers();
  const ready = detect.data?.find((answer) => answer.models.length > 0 || answer.needsKey);
  useEffect(() => {
    if (ready) onFound(ready);
  }, [ready]);

  if (detect.isPending || ready) {
    return <p role="status" className="m-0 flex items-center gap-2 text-sm text-subtext"><Spinner />{m.local_models_guide_looking()}</p>;
  }
  const empty = detect.data?.[0];
  const recommended = machine.data?.os === "macos" ? "lmstudio" : "ollama";
  const runtime = [picked, empty?.server.id, recommended].map((id) => RUNTIMES.find((server) => server.id === id)).find(Boolean) ?? LM_STUDIO;
  const lmStudio = runtime.id === "lmstudio";
  const memory = machine.data?.memBytes;
  const code = "self-start rounded-xs border border-border-variant bg-surface px-1 py-px font-mono text-xs";
  const steps = [
    {
      text: lmStudio ? m.local_models_guide_install_lmstudio() : m.local_models_guide_install_ollama(),
      extra: machine.data?.os === "linux" && !lmStudio ? <code className={code}>curl -fsSL https://ollama.com/install.sh | sh</code> : (
        <a href={lmStudio ? "https://lmstudio.ai/download" : "https://ollama.com/download"} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 text-accent-blue-strong underline">
          {m.local_models_guide_download({ name: runtime.label })}<ExternalLink size={12} aria-hidden="true" />
        </a>
      ),
    },
    {
      text: lmStudio ? m.local_models_guide_model_lmstudio() : m.local_models_guide_model_ollama(),
      extra: memory ? <span className="text-subtext">{m.local_models_guide_size({ memory: fmtBytes(memory), size: modelSize(memory) })}</span> : null,
    },
    {
      text: lmStudio ? m.local_models_guide_serve_lmstudio() : m.local_models_guide_serve_ollama(),
      extra: <code className={code}>{lmStudio ? "lms server start" : "ollama serve"}</code>,
    },
  ];

  return (
    <div className="grid gap-4">
      {detect.error ? <p role="alert" className="m-0 text-sm text-accent-red">{detect.error.message}</p>
        : <p className="m-0 text-sm">{empty ? m.local_models_guide_no_models({ name: empty.server.label }) : m.local_models_guide_none()}</p>}
      <OptionPicker
        choices={RUNTIMES.map((server) => ({ id: server.id, label: server.label }))}
        value={runtime.id}
        title={m.local_models_server()}
        header={m.local_models_server()}
        variant="field"
        floating
        dropDown
        disabled={disabled || detect.isFetching}
        renderLabel={(choice) => <>{choice.label}{choice.id === recommended && <span className="ms-1 text-xs font-normal text-subtext">{m.add_agent_recommended()}</span>}</>}
        renderIcon={(choice) => <EndpointMark server={SERVERS.find((server) => server.id === choice.id)} size={14} />}
        onSelect={setPicked}
      />
      <ol className="m-0 list-none space-y-3 p-0 text-sm">
        {steps.map((step, index) => (
          <li key={index} className="flex gap-3">
            <span className="grid size-5 shrink-0 place-items-center rounded-full border border-border text-xs text-subtext">{index + 1}</span>
            <span className="flex min-w-0 flex-col gap-1">{step.text}{step.extra}</span>
          </li>
        ))}
      </ol>
      <Button className="justify-self-start" onClick={() => void detect.refetch()} disabled={disabled || detect.isFetching}>
        {detect.isFetching ? <Spinner /> : <RefreshCw size={14} />}{m.onboarding_re_check()}
      </Button>
    </div>
  );
}
