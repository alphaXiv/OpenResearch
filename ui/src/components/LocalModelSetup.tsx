import { useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { useDialogFocus } from "./useDialogFocus";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link2, Plus, RefreshCw, Trash2, X } from "lucide-react";
import { connectLocalModel, discoverLocalModels, removeLocalModel, checkLocalModel, type LocalModelConnection } from "../api";
import { getLocalModelsQuery, getHarnessesQuery, refreshHarnesses } from "../queries/settings";
import { m } from "../paraglide/messages.js";
import { OptionPicker } from "./ModelPicker";
import lmStudioLogo from "../assets/lm-studio-logo.svg";
import ollamaLogo from "../assets/ollama-logo.png";
import omlxLogo from "../assets/omlx-logo.svg";
import { Badge, Button, ButtonLink, IconButton, Input, Spinner, showAlert } from "./ui";

const LM_STUDIO = { id: "lmstudio", label: "LM Studio", icon: lmStudioLogo, url: "http://127.0.0.1:1234/v1" };
const SERVERS = [
  LM_STUDIO,
  { id: "omlx", label: "oMLX", icon: omlxLogo, url: "http://127.0.0.1:8000/v1" },
  { id: "ollama", label: "Ollama", icon: ollamaLogo, url: "http://127.0.0.1:11434/v1" },
  { id: "openai-compatible", label: "Custom Endpoint", icon: null, url: "http://127.0.0.1:8000/v1" },
];

type ProviderDraft = {
  baseUrl: string;
  apiKey: string;
  models: string[];
  model: string | null;
  contextWindow: string;
  error: string | null;
};

export function LocalModelSetup({ installed, onConnected, dialogOnly = false, onClose }: {
  installed: boolean;
  onConnected?: (model: string) => void;
  dialogOnly?: boolean;
  onClose?: () => void;
}) {
  const queryClient = useQueryClient();
  const connections = useQuery({ ...getLocalModelsQuery(), enabled: !dialogOnly });
  const harnesses = useQuery(getHarnessesQuery());
  const opencode = harnesses.data?.find((harness) => harness.id === "opencode");
  const [open, setOpen] = useState(dialogOnly);
  const close = () => { setOpen(false); onClose?.(); };
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
  const save = useMutation({ mutationFn: async (request: Parameters<typeof connectLocalModel>[0]) => {
    const result = await connectLocalModel(request);
    const harnesses = await queryClient.fetchQuery({ ...getHarnessesQuery(), staleTime: 0 });
    if (!harnesses.find((h) => h.id === "opencode")?.models.some((model) => model.id === result.model)) {
      throw new Error(m.local_models_config_restricted());
    }
    return result;
  } });
  const remove = useMutation({ mutationFn: removeLocalModel });
  const context = Number(contextWindow);
  const busy = probe.isPending || save.isPending;
  const reset = (changes: Partial<ProviderDraft> = {}) => updateDraft({ models: [], model: null, error: null, ...changes });
  const check = async () => {
    reset();
    try {
      const result = await probe.mutateAsync({ baseUrl: baseUrl.trim(), apiKey });
      updateDraft({ models: result.models, model: result.models[0] ?? null });
    } catch (e) { updateDraft({ error: e instanceof Error ? e.message : String(e) }); }
  };
  const connect = async () => {
    if (!model) return;
    updateDraft({ error: null });
    try {
      const result = await save.mutateAsync({ name: server.label, baseUrl: baseUrl.trim(), apiKey, model, contextWindow: context });
      close();
      onConnected?.(result.model);
    } catch (e) { updateDraft({ error: e instanceof Error ? e.message : String(e) }); }
  };
  const disconnect = async (id: string) => {
    try { await remove.mutateAsync(id); }
    catch (e) { showAlert(e instanceof Error ? e.message : String(e), "error"); }
  };
  return (
    <>
    {!dialogOnly && (
    <section className="mt-4 space-y-3 border-t border-border pt-4" aria-label={m.local_models_title()}>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h3 className="text-base font-medium">{m.local_models_title()}</h3>
        <Button disabled={busy} onClick={() => setOpen(true)} aria-haspopup="dialog">
          <Plus size={14} />{m.local_models_add()}
        </Button>
      </div>
      {connections.error && <p className="text-sm text-accent-red" role="alert">{connections.error.message}</p>}
      <div className="space-y-3">
        {connections.data?.map((connection) => (
          <SavedLocalModel key={connection.id} connection={connection}
            harnessReady={opencode?.agentReady ?? false}
            availableModels={opencode?.models.map((model) => model.id) ?? []}
            checking={harnesses.isFetching}
            unknown={harnesses.isError || !harnesses.data}
            disabled={busy || remove.isPending}
            onRemove={() => void disconnect(connection.id)} />
        ))}
      </div>
    </section>
    )}
      {open && (
        <LocalModelDialog busy={busy} onClose={close} footer={
          <Button variant="primary" disabled={busy || !installed || (models.length > 0 && (!model || !Number.isSafeInteger(context) || context < 4096))} onClick={() => void (models.length > 0 ? connect() : check())}>
            {busy && <Spinner />}{models.length > 0 ? m.local_models_save() : m.local_models_check()}
          </Button>
        }>
          <div className="space-y-4">
            {!installed && <div className="text-sm text-text">
              <p>{m.local_models_install_opencode()}</p>
              <ButtonLink href="https://opencode.ai/docs/#install" target="_blank" rel="noreferrer">{m.local_models_get_opencode()}</ButtonLink>
            </div>}
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
            {models.length > 0 && <>
              <OptionPicker choices={models.map((id) => ({ id, label: id }))} value={model} searchPlaceholder={m.model_picker_search_models()} title={m.model_picker_model()} header={m.model_picker_model()} variant="field" floating dropDown disabled={busy} onSelect={(id) => { updateDraft({ model: id }); }} />
              <label className="block space-y-1 text-sm text-subtext">
                <span>{m.local_models_context()}</span>
                <Input type="number" min={4096} step={1024} value={contextWindow} disabled={busy} onChange={(e) => { updateDraft({ contextWindow: e.target.value }); }} />
              </label>
              <p className="text-sm text-subtext">{m.local_models_context_hint()}</p>

            </>}
            {error && <p className="text-sm text-accent-red" role="alert">{error}</p>}
          </div>
        </LocalModelDialog>
      )}
    </>
  );
}

function SavedLocalModel({ connection, harnessReady, availableModels, checking, unknown, disabled, onRemove }: {
  connection: LocalModelConnection;
  harnessReady: boolean;
  availableModels: string[];
  checking: boolean;
  unknown: boolean;
  disabled: boolean;
  onRemove: () => void;
}) {
  const probe = useMutation({ mutationFn: async () => {
    try { await checkLocalModel(connection.id); }
    finally { await refreshHarnesses(true); }
  } });
  const provider = SERVERS.find((item) => item.label === connection.name || (item.id === "openai-compatible" && ["OpenAI-compatible server", "Custom endpoint"].includes(connection.name)));
  const modelIds = Object.keys(connection.models);
  const ready = modelIds.length > 0 && harnessReady && modelIds.every((id) => availableModels.includes(`${connection.id}/${id}`));
  const pending = probe.isPending || checking;
  const statusUnknown = unknown;
  const connected = ready;
  return (
    <div className="@container min-w-0 px-4 py-4">
      <div className="flex flex-col items-stretch gap-3 @sm:flex-row @sm:items-center @sm:justify-between">
        <div className="flex min-w-0 flex-1 items-start gap-3">
          {provider?.icon ? <img src={provider.icon} alt="" width={20} height={20} className={`mt-0.5 size-5 shrink-0 object-contain${provider.id === "ollama" ? " dark:invert" : ""}`} /> : <Link2 size={20} className="mt-0.5 shrink-0" />}
          <div className="min-w-0 space-y-1">
            <div className="text-base font-medium break-words">{modelIds.join(", ")}</div>
            <div className="text-sm text-subtext break-all">{provider?.id === "openai-compatible" ? m.local_models_custom_endpoint() : provider?.label ?? connection.name}{provider?.id === "openai-compatible" && <span className="ml-1 text-xs font-normal text-subtext">{m.local_models_compatible_api()}</span>} · {connection.baseUrl}</div>
          </div>
        </div>
        <div className="flex shrink-0 flex-col items-end gap-1 self-end @sm:self-auto @lg:flex-row @lg:items-center @lg:gap-2">
          <span role="status"><Badge variant={pending || statusUnknown ? "default" : connected ? "success" : "warning"}>
            {pending ? m.common_checking() : statusUnknown ? m.settings_page_unknown() : connected ? m.local_models_connected() : m.model_picker_unavailable()}
          </Badge></span>
          <div className="flex items-center gap-1">
            <Button variant="ghost" disabled={disabled || probe.isPending} onClick={() => probe.mutate()}>
              <RefreshCw size={14} />{m.onboarding_re_check()}
            </Button>
            <IconButton aria-label={m.local_models_disconnect({ name: connection.name })} disabled={disabled || probe.isPending} onClick={onRemove}><Trash2 size={14} /></IconButton>
          </div>
        </div>
      </div>
      {probe.error && !connected && <p className="mt-2 text-sm text-accent-red" role="alert">{probe.error.message}</p>}
    </div>
  );
}

function LocalModelDialog({ busy, onClose, footer, children }: {
  busy: boolean;
  onClose: () => void;
  footer: ReactNode;
  children: ReactNode;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const close = () => { if (!busy) onClose(); };
  useDialogFocus(dialogRef, close, "input");
  return createPortal(
    <div className="fixed inset-0 z-200 flex items-center justify-center bg-modal-backdrop p-5" onClick={(event) => { if (event.target === event.currentTarget) close(); }}>
      <div ref={dialogRef} role="dialog" aria-modal="true" aria-labelledby="local-model-dialog-title" tabIndex={-1} className="relative flex max-h-full w-140 max-w-full flex-col rounded-xl border border-border bg-background shadow-modal">
        <div className="shrink-0 px-6 pt-5 pb-4 pe-14">
          <h2 id="local-model-dialog-title" className="m-0 text-xl font-medium">{m.local_models_add()}</h2>
        </div>
        <IconButton className="absolute end-3.5 top-3.5" aria-label={m.settings_page_cancel()} onClick={close} disabled={busy}><X size={16} /></IconButton>
        <div className="min-h-0 overflow-y-auto px-6 pb-2">{children}</div>
        <div className="flex shrink-0 justify-end gap-2 px-6 py-4">
          <Button onClick={close} disabled={busy}>{m.settings_page_cancel()}</Button>
          {footer}
        </div>
      </div>
    </div>,
    document.body,
  );
}
