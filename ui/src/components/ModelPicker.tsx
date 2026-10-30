import { EscapeScope } from "./EscapeScope";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Menu } from "@base-ui/react/menu";
import { Popover } from "@base-ui/react/popover";
import { DirectionProvider } from "@base-ui/react/direction-provider";
import { getHarnessesQuery } from "../queries/settings";
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { CircleAlert, Check, ChevronDown, ChevronRight, Plus, Settings, Zap } from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ButtonHTMLAttributes, type ReactNode, type RefObject } from "react";
import {
  fmtNumber,
  harnessModelLabel,
  modelLabel,
  reasoningFor,
  reconcileReasoning,
  reconcileServiceTier,
  REASONING_DEFAULT_ID,
  serviceTiersFor,
  type Harness,
  type HarnessId,
  type OptionChoice,
  type AgentSelection,
  type Autonomy,
  DEFAULT_AUTONOMY,
} from "../api";
import { renderNote } from "./agentNote";
import type { NativeOption } from "../acp";
import { HarnessLogo } from "./HarnessLogo";

import { IconButton, MenuItem, SwitchIndicator, Tooltip } from "./ui";
import { cn } from "./ui/cn";

import { LocalModelDialog } from "./LocalModelSetup";
import { showCustomDialog } from "../dialog";
import { formatCodexModel, groupModels, resolveHarnessDefaultModel } from "../modelSelection";

const MODEL_GROUP_CLASS_NAME = [
  "model-group flex items-center justify-between gap-2",
  "text-sm font-medium text-text pt-2.5 px-2 pb-1.5",
].join(" ");

const MODEL_MORE_CLASS_NAME = [
  "model-more [&_code]:font-mono [&_code]:text-xs",
  "[&_code]:bg-surface [&_code]:border [&_code]:border-border-variant",
  "[&_code]:rounded-xs [&_code]:py-px [&_code]:px-1 [&_code]:whitespace-nowrap",
  "pt-1 px-2 pb-2 text-sm text-muted",
].join(" ");

export type ModelSelection = AgentSelection;

export const HARNESS_LABELS: Record<HarnessId, string> = {
  "claude-code": "Claude Code",
  codex: "Codex",
  opencode: "OpenCode",
  cursor: "Cursor",
  antigravity: "Google Antigravity",
};

export function harnessLabel(id: HarnessId, name?: string | null): string {
  return name || HARNESS_LABELS[id] || (id.startsWith("acp:") ? "ACP" : id);
}

/** First harness that can actually run — the fallback when nothing is picked.
 * Seeds mode/reasoning from that harness's advertised defaults. */
export function defaultSelection(harnesses: Harness[]): ModelSelection | null {
  const ready = harnesses.find((h) => h.agentReady);
  if (!ready) return null;
  const model = resolveHarnessDefaultModel(ready);
  return {
    harness: ready.id,
    model,
    serviceTier: reconcileServiceTier(ready, model, null),
    permissionMode: ready.options?.defaultPermissionMode ?? null,
    reasoningLevel: reasoningFor(ready, model).defaultId,
  };
}

/** Close-on-outside-click + open state shared by the composer dropdowns (and
 * the session-rail menus). */
export function usePopover(triggerRef?: RefObject<HTMLButtonElement | null>) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (e.target instanceof Node && !ref.current?.contains(e.target) && !triggerRef?.current?.contains(e.target)) setOpen(false);
    };
    document.addEventListener("mousedown", onDown, true);
    return () => {
      document.removeEventListener("mousedown", onDown, true);
    };
  }, [open, triggerRef]);
  return { open, setOpen, ref };
}

const SETTINGS_ROW =
  "flex min-h-8 w-full items-center justify-between gap-2 rounded-sm px-2 py-1.5 text-start text-sm text-text outline-none transition-[background,color] duration-120 ease-standard data-[highlighted]:bg-surface data-[popup-open]:bg-surface focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2";

function SettingsSubmenu({
  label,
  detail,
  choices,
  value,
  defaultId,
  onSelect,
}: {
  label: string;
  detail?: string;
  choices: OptionChoice[];
  value?: string | null;
  defaultId?: string | null;
  onSelect: (id: string) => void;
}) {
  const [open, setOpen] = useState(false);

  return (
    <Menu.SubmenuRoot open={open} onOpenChange={setOpen}>
      <Menu.SubmenuTrigger delay={0} label={label} className={SETTINGS_ROW}>
        <span>{label}</span>
        <span className="ms-auto max-w-36 truncate text-muted">{detail}</span>
        <ChevronRight size={14} aria-hidden="true" className="shrink-0 text-muted rtl:rotate-180" />
      </Menu.SubmenuTrigger>
      <Menu.Portal>
        <Menu.Positioner className="z-50" alignOffset={-6} collisionPadding={8} positionMethod="fixed">
          <Menu.Popup
            aria-label={label}
            className="overflow-y-auto rounded-lg border border-border bg-background p-1.5 shadow-dropdown outline-none"
            style={{
              width: "min(320px, calc(100vw - 16px))",
              maxHeight: "min(360px, var(--available-height))",
            }}
          >
            <Menu.RadioGroup
              value={value}
              onValueChange={(id) => {
                if (typeof id === "string") onSelect(id);
              }}
            >
              {choices.map((choice) => (
                <Menu.RadioItem
                  key={choice.id}
                  value={choice.id}
                  closeOnClick
                  label={choice.label}
                  className={cn(SETTINGS_ROW, "items-start")}
                >
                  <span className="flex min-w-0 flex-col items-start gap-0.5">
                    <span>
                      {choice.label}
                      {choice.id === defaultId && (
                        <span className="text-muted"> {m.model_picker_default()}</span>
                      )}
                    </span>
                    {choice.description && (
                      <span className="text-sm font-normal leading-snug text-muted">
                        {choice.description}
                      </span>
                    )}
                  </span>
                  <Menu.RadioItemIndicator className="shrink-0">
                    <Check size={14} aria-hidden="true" />
                  </Menu.RadioItemIndicator>
                </Menu.RadioItem>
              ))}
            </Menu.RadioGroup>
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
      <EscapeScope enabled={open} onEscape={() => setOpen(false)} />
    </Menu.SubmenuRoot>
  );
}

/** Composer selector: pick the harness + model new sessions (and same-harness
 * turns) run on. Groups mirror the Harnesses settings tab. */
export function ModelPicker({
  value,
  onSelect,
  onOpenSettings,
  permissionChoices = [],
  defaultPermissionId,
  onSelectPermission,
  reasoningChoices = [],
  defaultReasoningId,
  onSelectReasoning,
  autonomy,
  onSelectAutonomy,
  lockHarness = false,
  openRequest = 0,
  className,
  sessionHarness,
  nativeOptions = [],
  onSelectNativeOption,
  onStartHarness,
}: {
  value: ModelSelection | null;
  onSelect: (value: ModelSelection) => void;
  onOpenSettings: () => void;
  permissionChoices?: OptionChoice[];
  defaultPermissionId?: string | null;
  onSelectPermission?: (id: string) => void;
  reasoningChoices?: OptionChoice[];
  defaultReasoningId?: string | null;
  onSelectReasoning?: (id: string) => void;
  autonomy?: Autonomy;
  onSelectAutonomy?: (autonomy: Autonomy) => void;
  lockHarness?: boolean;
  openRequest?: number;
  className?: string;
  sessionHarness?: Harness;
  nativeOptions?: NativeOption[];
  onSelectNativeOption?: (optionId: string, value: string) => void;
  onStartHarness?: (harness: Harness) => void;
}) {
  const { data: catalog = EMPTY_HARNESSES } = useQuery(getHarnessesQuery());
  const harnesses = useMemo(
    () => (sessionHarness ? [sessionHarness, ...catalog.filter((h) => h.id !== sessionHarness.id)] : catalog),
    [catalog, sessionHarness],
  );
  const queryClient = useQueryClient();
  const localModelConnected = useRef<(model: string) => void>(() => {});
  useEffect(() => () => { localModelConnected.current = () => {}; }, []);
  const [menu, setMenu] = useState<"model" | "settings" | null>(null);
  const [legacyMenu, setLegacyMenu] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const modelTriggerRef = useRef<HTMLButtonElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const handledOpenRequest = useRef(openRequest);

  const close = () => {
    setMenu(null);
    setFilter("");
    modelTriggerRef.current?.focus();
  };

  useEffect(() => {
    if (openRequest === handledOpenRequest.current) return;
    handledOpenRequest.current = openRequest;
    setFilter("");
    setMenu("model");
    searchRef.current?.focus();
  }, [openRequest]);

  const groups = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const current = harnesses.find((harness) => harness.id === value?.harness);
    const shown =
      lockHarness && value
        ? current
          ? [current]
          : []
        : current
          ? [current, ...harnesses.filter((harness) => harness.id !== current.id)]
          : harnesses;
    return shown.map((harness) => {
      let models = harness.models.filter((model) => harness.id !== "codex" || model.id !== "default");
      if (q)
        models = models.filter((model) =>
          `${model.id} ${harnessModelLabel(model)}`.toLowerCase().includes(q),
        );
      else if (
        harness.id === "opencode" ||
        harness.id === "cursor" ||
        harness.id === "antigravity" ||
        harness.id.startsWith("acp:")
      )
        models = models.slice(0, 5);
      const count = harness.models.filter((model) => harness.id !== "codex" || model.id !== "default").length;
      const grouped = q ? { current: models, legacy: [] } : groupModels(harness.id, models);
      return { harness, models: grouped.current, legacy: grouped.legacy, hidden: q ? 0 : count - models.length };
    });
  }, [harnesses, filter, lockHarness, value]);

  const pick = (harness: Harness, model: string | null) => {
    const previous = value?.harness === harness.id ? value : null;

    onSelect({
      harness: harness.id,
      model,
      serviceTier: reconcileServiceTier(harness, model, previous?.serviceTier),
      permissionMode: previous ? previous.permissionMode : (harness.options?.defaultPermissionMode ?? null),
      reasoningLevel: reconcileReasoning(
        harness,
        model,
        previous && harness.models.some((entry) => entry.id === model) ? previous.reasoningLevel : null,
      ),
    });

    close();
  };

  localModelConnected.current = (model) => {
    const harness = queryClient.getQueryData(getHarnessesQuery().queryKey)?.find((entry) => entry.id === "opencode");
    if (harness && (!lockHarness || value?.harness === "opencode")) pick(harness, model);
  };

  const selectedHarness = harnesses.find((harness) => harness.id === value?.harness);
  const selected = selectedHarness?.models.find((model) => model.id === value?.model);
  const displayModel = (harness: Harness, model: Harness["models"][number]) => harness.id === "codex" ? formatCodexModel(harnessModelLabel(model)) : harnessModelLabel(model);
  const defaultModel = selectedHarness?.models.find((model) => model.id === resolveHarnessDefaultModel(selectedHarness));
  const label = value
    ? value.model
      ? selected
        ? displayModel(selectedHarness!, selected)
        : value.harness.startsWith("acp:") && value.model === "default"
          ? m.model_picker_default_model()
          : `${value.harness === "codex" ? formatCodexModel(value.model) : modelLabel(value.model)} · ${m.model_picker_unverified()}`
      : defaultModel && selectedHarness ? displayModel(selectedHarness, defaultModel) : m.model_picker_default_model()
    : m.model_picker_model();
  const effectiveReasoningId = value?.reasoningLevel ?? defaultReasoningId ?? reasoningChoices[0]?.id;
  const reasoningLabel = reasoningChoices.find((choice) => choice.id === effectiveReasoningId)?.label;
  const effectivePermissionId = value?.permissionMode ?? defaultPermissionId ?? permissionChoices[0]?.id;
  const permissionLabel = permissionChoices.find((choice) => choice.id === effectivePermissionId)?.label;
  const effectiveServiceTier = reconcileServiceTier(selectedHarness, value?.model, value?.serviceTier);
  const fastChoice = serviceTiersFor(selectedHarness, value?.model).find((choice) => choice.id === "priority");
  const autonomyChoices: (OptionChoice & { id: Autonomy })[] = [
    {
      id: "agentic",
      label: m.model_picker_autonomy_agentic(),
      description: m.model_picker_autonomy_agentic_description(),
    },
    {
      id: "copilot",
      label: m.model_picker_autonomy_copilot(),
      description: m.model_picker_autonomy_copilot_description(),
    },
  ];
  const autonomyLabel = autonomyChoices.find((choice) => choice.id === autonomy)?.label;
  const moveFocus = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const items = Array.from(
      event.currentTarget.querySelectorAll<HTMLElement>("input, .model-menu-list button:not([disabled])"),
    );
    if (!items.length) return;

    event.preventDefault();
    const index = items.findIndex((item) => item === document.activeElement);
    items[(index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
  };

  const hasSettings =
    reasoningChoices.length > 0 ||
    nativeOptions.length > 0 ||
    !!fastChoice ||
    permissionChoices.length > 0 ||
    !!onSelectAutonomy;

  useEffect(() => {
    if (menu === "settings" && !hasSettings) setMenu(null);
    if (menu !== "model") setLegacyMenu(null);
  }, [menu, hasSettings]);

  return (
    <DirectionProvider direction={document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
      <div
        className="model-picker relative inline-flex min-w-0 items-center gap-0.5"
        data-onboarding="model-picker"
      >
        <Popover.Root
          open={menu === "model"}
          onOpenChange={(open) => {
            setFilter("");
            setMenu((current) => (open ? "model" : current === "model" ? null : current));
          }}
        >
          <Tooltip interactive content={label} className="min-w-0 max-w-full rounded-md">
            <Popover.Trigger
              ref={modelTriggerRef}
              type="button"
              className={cn(
                "composer-pill inline-flex h-8 min-w-0 max-w-full items-center gap-1 rounded-md px-2 text-sm text-text whitespace-nowrap transition-[background,color] duration-150 ease-standard hover:bg-surface",
                className,
              )}
              aria-label={m.a11y_chat_model({
                label:
                  effectiveServiceTier === "priority" ? `${m.model_picker_fast_speed()} ${label}` : label,
              })}
              onKeyDown={(event) => {
                if (event.key === "ArrowDown") {
                  event.preventDefault();
                  setFilter("");
                  setMenu("model");
                  searchRef.current?.focus();
                }
              }}
            >
              {effectiveServiceTier === "priority" ? (
                <Zap size={14} fill="currentColor" aria-hidden="true" />
              ) : value?.harness ? (
                <HarnessLogo harness={value.harness} logo={selectedHarness?.logo} size={14} />
              ) : null}
              <span className="model-picker-label min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">
                {label}
              </span>
              <ChevronDown size={14} className="shrink-0 text-muted" />
            </Popover.Trigger>
          </Tooltip>
          <Popover.Portal>
            <Popover.Positioner
              className="z-50"
              side="top"
              align="end"
              sideOffset={8}
              collisionPadding={8}
              collisionAvoidance={{ fallbackAxisSide: "none" }}
              positionMethod="fixed"
            >
              <Popover.Popup
                initialFocus={searchRef}
                onKeyDown={moveFocus}
                aria-label={m.model_picker_model()}
                className="model-menu flex flex-col overflow-hidden rounded-lg border border-border bg-background shadow-dropdown outline-none"
                style={{
                  width: "min(288px, calc(100vw - 16px))",
                  maxHeight: "min(400px, var(--available-height))",
                }}
              >
                <div className="flex shrink-0 items-center border-b border-border-variant">
                  <input
                    ref={searchRef}
                    type="text"
                    aria-label={m.model_picker_search_or_enter_id()}
                    placeholder={m.model_picker_search_or_enter_id()}
                    value={filter}
                    onChange={(event) => setFilter(event.target.value)}
                    className="min-w-0 flex-1 border-0 bg-transparent px-3 py-2 text-sm outline-none"
                  />
                  <IconButton
                    type="button"
                    size="small"
                    aria-label={m.settings_page_harnesses()}
                    onClick={() => {
                      close();
                      onOpenSettings();
                    }}
                  >
                    <Settings size={16} aria-hidden="true" />
                  </IconButton>
                </div>
                <div className="model-menu-list min-h-0 overflow-x-hidden overflow-y-auto p-1.5 [&_button]:min-w-0 [&_button]:[overflow-wrap:anywhere] [&_button>span]:min-w-0">
                  {groups.map(({ harness, models, legacy, hidden }) => (
                    <div key={harness.id}>
                      {harness.agentReady ? (
                        <div className={MODEL_GROUP_CLASS_NAME}>
                          <span className="inline-flex items-center gap-1.5">
                            <HarnessLogo harness={harness.id} logo={harness.logo} size={14} />
                            {harness.name}
                          </span>
                        </div>
                      ) : harness.catalogPending ? (
                        <div className={MODEL_GROUP_CLASS_NAME}>
                          <span className="inline-flex items-center gap-1.5">
                            <HarnessLogo harness={harness.id} logo={harness.logo} size={14} />
                            {harness.name}
                          </span>
                          <span className="font-normal text-muted">{m.onboarding_checking()}</span>
                        </div>
                      ) : (
                        <MenuItem type="button" className="font-medium" onClick={() => { close(); onOpenSettings(); }}>
                          <span className="inline-flex min-w-0 items-center gap-1.5">
                            <HarnessLogo harness={harness.id} logo={harness.logo} size={14} />
                            {harness.name}
                          </span>
                          <span className="inline-flex items-center gap-1 font-normal text-accent-amber">
                            <CircleAlert size={14} aria-hidden="true" className="shrink-0" />
                            {!harness.installed ? m.settings_page_not_installed()
                              : harness.installBroken ? m.settings_page_install_broken()
                              : harness.needsConfigRepair || harness.authCheckFailed ? m.settings_page_unable_to_verify()
                              : harness.authState === "needsLogin" ? m.model_picker_needs_login()
                              : harness.authState === "unsupported" ? m.settings_page_update_required()
                              : m.settings_page_unable_to_verify()}
                          </span>
                        </MenuItem>
                      )}
                      {harness.agentReady && (
                        <>
                          {!filter.trim() && harness.id !== "codex" && !(harness.id === "claude-code" && harness.models.length > 0) && !harness.id.startsWith("acp:") && (
                            <MenuItem
                              type="button"
                              className="ps-6"
                              aria-pressed={value?.harness === harness.id && value.model === null}
                              onClick={() => pick(harness, null)}
                            >
                              <span>{m.model_picker_default_model()}</span>
                              {value?.harness === harness.id && value.model === null && <Check size={14} />}
                            </MenuItem>
                          )}
                          {harness.models.length === 0 && harness.agentNote && (
                            <div className={MODEL_MORE_CLASS_NAME}>{renderNote(harness.agentNote)}</div>
                          )}
                          {models.map((model) => (
                            <Tooltip key={model.id} interactive content={model.id} className="w-full rounded-sm">
                              <MenuItem
                                type="button"
                                className="ps-6"
                                aria-description={model.id}
                                aria-pressed={value?.harness === harness.id && (value.model === model.id || (value.model === null && model.id === resolveHarnessDefaultModel(harness)))}
                                onClick={() => pick(harness, model.id)}
                              >
                                <span>{displayModel(harness, model)}</span>
                                {value?.harness === harness.id && (value.model === model.id || (value.model === null && model.id === resolveHarnessDefaultModel(harness))) && (
                                  <Check size={14} />
                                )}
                              </MenuItem>
                            </Tooltip>
                          ))}
                          {legacy.length > 0 && (
                            <Menu.Root open={legacyMenu === harness.id} onOpenChange={(open) => setLegacyMenu(open ? harness.id : null)}>
                              <Menu.Trigger openOnHover delay={200} closeDelay={300} className={cn(SETTINGS_ROW, "ps-6")}>
                                <span>{m.model_picker_legacy()}</span>
                                <ChevronRight size={14} aria-hidden="true" className="text-muted rtl:rotate-180" />
                              </Menu.Trigger>
                              <Menu.Portal>
                                <Menu.Positioner side="right" align="start" sideOffset={4} collisionPadding={8} className="z-50">
                                  <Menu.Popup onKeyDown={(event) => { if (event.key !== "Escape") event.stopPropagation(); }} aria-label={m.model_picker_legacy()} className="max-h-80 overflow-y-auto rounded-lg border border-border bg-background p-1.5 shadow-dropdown outline-none">
                                    {legacy.map((model) => (
                                      <Menu.Item key={model.id} className={SETTINGS_ROW} onClick={() => pick(harness, model.id)}>
                                        <span>{displayModel(harness, model)}</span>
                                        {value?.harness === harness.id && (value.model === model.id || (value.model === null && model.id === resolveHarnessDefaultModel(harness))) && <Check size={14} aria-hidden="true" />}
                                      </Menu.Item>
                                    ))}
                                  </Menu.Popup>
                                </Menu.Positioner>
                              </Menu.Portal>
                            </Menu.Root>
                          )}
                          {filter.trim().length > 0 &&
                            !harness.id.startsWith("acp:") &&
                            !harness.models.some((model) => model.id === filter.trim()) && (
                              <MenuItem
                                type="button"
                                className="ps-6"
                                onClick={() => pick(harness, filter.trim())}
                              >
                                <span>
                                  {m.model_picker_use_id({ id: ltr(filter.trim()) })} ·{" "}
                                  {m.model_picker_unverified()}
                                </span>
                              </MenuItem>
                            )}
                        </>
                      )}
                      {harness.agentReady && harness.id === "opencode" && (
                        <MenuItem
                          type="button"
                          className="font-medium"
                          aria-haspopup="dialog"
                          onClick={() => {
                            close();
                            showCustomDialog({
                              component: LocalModelDialog,
                              props: { onConnected: (model) => localModelConnected.current(model) },
                              onConfirm: (connect) => connect(),
                              onCancel: "close",
                            });
                          }}
                        >
                          <span className="inline-flex items-center gap-1.5">
                            {m.model_picker_add_local_model()}
                            <Plus size={14} aria-hidden="true" />
                          </span>
                        </MenuItem>
                      )}
                      {harness.agentReady && hidden > 0 && (
                        <div className={MODEL_MORE_CLASS_NAME}>
                          {m.model_picker_more({ count: fmtNumber(hidden) })}
                        </div>
                      )}
                    </div>
                  ))}
                  {harnesses.length === 0 && (
                    <div className={MODEL_MORE_CLASS_NAME}>{m.model_picker_detecting_harnesses()}</div>
                  )}
                  {lockHarness &&
                    onStartHarness &&
                    harnesses.some((harness) => harness.id !== value?.harness && harness.agentReady) && (
                      <>
                        <div role="separator" className="m-1 h-px bg-border-variant" />
                        {harnesses
                          .filter((harness) => harness.id !== value?.harness && harness.agentReady)
                          .map((harness) => (
                            <Tooltip
                              key={harness.id}
                              interactive
                              content={m.model_picker_new_chat_harness()}
                              className="w-full rounded-sm"
                            >
                              <MenuItem
                                type="button"
                                aria-description={m.model_picker_new_chat_harness()}
                                onClick={() => {
                                  close();
                                  onStartHarness(harness);
                                }}
                              >
                                <span className="inline-flex items-center gap-1.5">
                                  <HarnessLogo harness={harness.id} logo={harness.logo} size={14} />
                                  {harness.name}
                                </span>
                                <ChevronRight size={14} className="text-muted" />
                              </MenuItem>
                            </Tooltip>
                          ))}
                      </>
                    )}
                </div>
              </Popover.Popup>
            </Popover.Positioner>
          </Popover.Portal>
        </Popover.Root>
        {hasSettings && (
          <Menu.Root
            open={menu === "settings"}
            modal={false}
            onOpenChange={(open) =>
              setMenu((current) => (open ? "settings" : current === "settings" ? null : current))
            }
          >
            <Menu.Trigger
              className="composer-pill inline-flex h-8 shrink-0 items-center gap-1 rounded-md px-2 text-sm text-muted transition-[background,color] duration-150 ease-standard hover:bg-surface"
              aria-label={
                reasoningLabel
                  ? `${m.settings_page_settings()}: ${reasoningLabel}`
                  : m.settings_page_settings()
              }
            >
              {reasoningLabel ? <span>{reasoningLabel}</span> : <Settings size={14} aria-hidden="true" />}
              <ChevronDown size={14} className="shrink-0" />
            </Menu.Trigger>
            <Menu.Portal>
              <Menu.Positioner
                className="z-50"
                side="top"
                align="end"
                sideOffset={8}
                collisionPadding={8}
                positionMethod="fixed"
              >
                <Menu.Popup
                  aria-label={m.settings_page_settings()}
                  className="model-settings-menu overflow-y-auto rounded-lg border border-border bg-background p-1 shadow-dropdown outline-none"
                  style={{
                    width: "min(288px, calc(100vw - 16px))",
                    maxHeight: "min(400px, var(--available-height))",
                  }}
                >
                  <Menu.RadioGroup
                    value={effectiveReasoningId}
                    onValueChange={(id) => {
                      if (typeof id === "string") onSelectReasoning?.(id);
                    }}
                  >
                    {reasoningChoices.map((choice) => (
                      <Menu.RadioItem
                        key={choice.id}
                        value={choice.id}
                        closeOnClick
                        label={choice.label}
                        className={SETTINGS_ROW}
                      >
                        <span>
                          {choice.label}
                          {choice.id === defaultReasoningId && (
                            <span className="text-muted"> {m.model_picker_default()}</span>
                          )}
                        </span>
                        <Menu.RadioItemIndicator>
                          <Check size={14} />
                        </Menu.RadioItemIndicator>
                      </Menu.RadioItem>
                    ))}
                  </Menu.RadioGroup>
                  {nativeOptions.map((option) => (
                    <SettingsSubmenu
                      key={option.id}
                      label={option.name}
                      detail={option.choices.find((choice) => choice.id === option.currentValue)?.label}
                      choices={option.choices}
                      value={option.currentValue}
                      onSelect={(id) => onSelectNativeOption?.(option.id, id)}
                    />
                  ))}
                  {(reasoningChoices.length > 0 || nativeOptions.length > 0) &&
                    (fastChoice || permissionChoices.length > 0 || onSelectAutonomy) && (
                      <Menu.Separator className="m-1 h-px bg-border-variant" />
                    )}
                  {fastChoice && (
                    <Menu.CheckboxItem
                      className={cn(SETTINGS_ROW, "min-h-9")}
                      checked={effectiveServiceTier === fastChoice.id}
                      onCheckedChange={(checked) => {
                        if (value) onSelect({ ...value, serviceTier: checked ? fastChoice.id : "default" });
                      }}
                    >
                      <span>{m.model_picker_fast_mode()}</span>
                      <SwitchIndicator checked={effectiveServiceTier === fastChoice.id} aria-hidden="true" />
                    </Menu.CheckboxItem>
                  )}
                  {fastChoice && (permissionChoices.length > 0 || onSelectAutonomy) && (
                    <Menu.Separator className="m-1 h-px bg-border-variant" />
                  )}
                  {permissionChoices.length > 0 && (
                    <SettingsSubmenu
                      label={m.model_picker_permissions()}
                      detail={permissionLabel}
                      choices={permissionChoices}
                      value={effectivePermissionId}
                      defaultId={defaultPermissionId}
                      onSelect={(id) => onSelectPermission?.(id)}
                    />
                  )}
                  {onSelectAutonomy && (
                    <SettingsSubmenu
                      label={m.model_picker_autonomy()}
                      detail={autonomyLabel}
                      choices={autonomyChoices}
                      value={autonomy ?? DEFAULT_AUTONOMY}
                      defaultId={DEFAULT_AUTONOMY}
                      onSelect={(id) => {
                        const choice = autonomyChoices.find((entry) => entry.id === id);
                        if (choice) onSelectAutonomy(choice.id);
                      }}
                    />
                  )}
                </Menu.Popup>
              </Menu.Positioner>
            </Menu.Portal>
          </Menu.Root>
        )}
      </div>
      <EscapeScope enabled={menu !== null} onEscape={() => setMenu(null)} />
      <EscapeScope enabled={menu === "model" && legacyMenu !== null} onEscape={() => setLegacyMenu(null)} />
    </DirectionProvider>
  );
}

/** A compact single-axis dropdown (permission mode or reasoning level). Renders
 * nothing when the harness advertises no choices for this axis. Mirrors the
 * Claude Code composer menu: a header, the current-default row pinned at top
 * with a "· Default" note, then the full numbered list. */
export function OptionPicker({
  triggerIcon,
  triggerLabel,
  onTriggerClick,
  menuAction,
  choices,
  value,
  defaultId,
  header,
  align = "left",
  dropDown = false,
  disabled = false,
  variant = "pill",
  title,
  numbered = false,
  searchPlaceholder,
  renderIcon,
  renderLabel,
  floating = false,
  openOnHover = false,
  onSelect,
  className,
}: {
  triggerIcon?: ReactNode;
  triggerLabel?: string;
  onTriggerClick?: () => void;
  menuAction?: { label: string; onSelect: () => void };
  choices: OptionChoice[];
  value: string | null;
  /** The harness's default id — pinned at top of the menu and used when
   * `value` is null. */
  defaultId?: string | null;
  /** Group header (e.g. "Mode"). */
  header?: string;
  align?: "left" | "right";
  dropDown?: boolean;
  disabled?: boolean;
  /** `pill` = compact composer control; `bare` = text-only; `field` = settings input. */
  variant?: "pill" | "bare" | "field";
  title?: string;
  /** Show 1-based number hints on the right (like the mode menu). */
  numbered?: boolean;
  searchPlaceholder?: string;
  renderIcon?: (choice: OptionChoice) => ReactNode;
  renderLabel?: (choice: OptionChoice) => ReactNode;
  floating?: boolean;
  openOnHover?: boolean;
  onSelect: (id: string) => void;
  className?: string;
}) {
  const buttonRef = useRef<HTMLButtonElement>(null);
  const { open, setOpen, ref } = usePopover(buttonRef);
  const menuRef = useRef<HTMLDivElement>(null);
  const [filter, setFilter] = useState("");
  const openedByHover = useRef(false);
  const touchTrigger = useRef(false);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(closeTimer.current), []);
  useLayoutEffect(() => {
    const menu = menuRef.current;
    const anchor = ref.current;
    if (!open || !floating || !menu || !anchor) return;
    if (!menu.matches(":popover-open")) {
      menu.showPopover();
      menu.querySelector("input")?.focus();
    }
    const position = () => {
      const bounds = anchor.getBoundingClientRect();
      const below = window.innerHeight - bounds.bottom - 12;
      const above = bounds.top - 12;
      const needed = Math.min(380, menu.scrollHeight);
      const downward = dropDown ? below >= needed || below >= above : above < needed && below > above;
      menu.style.width = `${bounds.width}px`;
      menu.style.minWidth = "0";
      menu.style.maxHeight = `${Math.max(0, Math.min(380, downward ? below : above))}px`;
      menu.style.left = `${bounds.left}px`;
      menu.style.top = `${downward ? bounds.bottom + 4 : bounds.top - menu.getBoundingClientRect().height - 4}px`;
    };
    position();
    window.addEventListener("resize", position);
    window.addEventListener("scroll", position, true);
    return () => {
      window.removeEventListener("resize", position);
      window.removeEventListener("scroll", position, true);
    };
  }, [open, floating, dropDown, filter, choices.length, ref]);
  if (choices.length === 0 && !menuAction) return null;

  const effectiveId = value ?? defaultId ?? choices[0]?.id ?? null;
  const current = choices.find((c) => c.id === effectiveId);
  const defaultChoice = choices.find((c) => c.id === defaultId);
  // Only the `default` sentinel gets pinned above a separator — it isn't a
  // point on the tier ramp, it's a different kind of choice (Claude's Adaptive,
  // opencode's "let the model decide"). A *concrete* default (codex's resolved
  // tier, a permission mode) stays inline in its natural position with just
  // the "· Default" marker, so the ramp reads in order.
  const pinned =
    variant === "bare" && defaultChoice?.id === REASONING_DEFAULT_ID
      ? defaultChoice
      : undefined;
  const rest = pinned ? choices.filter((c) => c.id !== pinned.id) : choices;
  const visible = rest.filter((c) => `${c.label} ${c.id}`.toLowerCase().includes(filter.toLowerCase()));
  const label = current?.label ?? choices[0]?.label ?? "";

  const activateTrigger = () => {
    openedByHover.current = false;
    setFilter("");
    if (onTriggerClick && !touchTrigger.current) {
      setOpen(false);
      onTriggerClick();
    } else {
      setOpen((value) => !value);
    }
  };

  const choose = (id: string) => {
    onSelect(id);
    setOpen(false);
    buttonRef.current?.focus();
  };

  const iconTriggerProps: Omit<ButtonHTMLAttributes<HTMLButtonElement>, "title"> = {
    "aria-label": triggerLabel ?? title ?? header ?? label,
    "aria-haspopup": "menu",
    "aria-expanded": open,
    disabled,
    onPointerDown: (event) => { touchTrigger.current = event.pointerType === "touch"; },
    onClick: activateTrigger,
    onKeyDown: (event) => {
      touchTrigger.current = false;
      if (!onTriggerClick || event.key !== "ArrowDown") return;
      event.preventDefault();
      openedByHover.current = false;
      setFilter("");
      setOpen(true);
      menuRef.current?.querySelector("input")?.focus();
    },
  };

  return (
    <div className={`option-picker relative inline-flex${variant === "field" || triggerLabel ? " w-full" : ""}`} ref={ref}
      onPointerEnter={(event) => {
        if (!openOnHover || disabled || event.pointerType !== "mouse") return;
        clearTimeout(closeTimer.current);
        if (!open) {
          openedByHover.current = true;
          setFilter("");
          setOpen(true);
        }
      }}
      onPointerLeave={() => {
        if (!openOnHover) return;
        closeTimer.current = setTimeout(() => {
          if (!menuRef.current?.contains(document.activeElement)) setOpen(false);
        }, 150);
      }}
    >
      {triggerIcon ? (
        triggerLabel ? (
          <MenuItem {...iconTriggerProps} ref={buttonRef} active={open} className={cn("min-h-7 py-1", className)}>
            <span className="flex items-center gap-4">{triggerIcon}{triggerLabel}</span>
          </MenuItem>
        ) : (
          <IconButton {...iconTriggerProps} ref={buttonRef} size="small" active={open} className={className}>
            {triggerIcon}
          </IconButton>
        )
      ) : (
      <button
        ref={buttonRef}
        type="button"
        className={cn(
          variant === "field"
            ? "inline-flex h-9 w-full items-center justify-between gap-2 rounded-md border border-border bg-background px-3 text-sm font-normal text-text transition-colors duration-120 ease-standard hover:bg-surface disabled:opacity-45"
            : `inline-flex h-8 items-center rounded-md transition-[background,color] duration-150 ease-standard hover:bg-surface ${variant === "pill" ? "composer-pill gap-1 px-2 text-sm text-text whitespace-nowrap" : "composer-bare gap-1 px-1 text-sm text-text"}`,
          className,
        )}
        title={title}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={disabled}
        onClick={activateTrigger}
      >
        <span className="inline-flex min-w-0 items-center gap-2">
          {current && renderIcon?.(current)}
          <span className="truncate">{current ? renderLabel?.(current) ?? label : label}</span>
        </span>
        <ChevronDown size={12} />
      </button>
      )}
      {open && (
        <div ref={menuRef} popover={floating ? "manual" : undefined}
          style={floating ? { position: "fixed", inset: "auto", margin: 0 } : undefined}
          className={`option-menu absolute bottom-[calc(100%_+_8px)] start-0 max-h-95 flex flex-col bg-background border border-border rounded-lg shadow-menu z-50 overflow-hidden min-w-47.5 p-1.5 [&.align-right]:start-auto [&.align-right]:end-0 [&.drop-down]:bottom-auto [&.drop-down]:top-[calc(100%_+_4px)] [&.session-menu]:start-auto [&.session-menu]:end-1.5 [&.session-menu]:top-[calc(100%_-_2px)] [&.session-menu]:min-w-35 ${choices.some((choice) => choice.description) ? "min-w-80" : ""} ${variant === "field" ? "min-w-full text-text" : ""} ${align === "right" ? "align-right" : ""} ${dropDown ? "drop-down" : ""}`}>
          {header && <div className={MODEL_GROUP_CLASS_NAME}>{header}</div>}
          {menuAction && <MenuItem type="button" onClick={() => {
            setOpen(false);
            buttonRef.current?.focus();
            menuAction.onSelect();
          }}><span className="flex items-center gap-2"><Plus size={14} />{menuAction.label}</span></MenuItem>}
          {searchPlaceholder && <input autoFocus={!openedByHover.current} aria-label={searchPlaceholder} placeholder={searchPlaceholder} value={filter} onChange={(e) => setFilter(e.target.value)} className={cn("mb-2 shrink-0 border-b border-border bg-background px-2 py-2 text-sm outline-none", menuAction && "mt-1.5")} />}
          <div className="min-h-0 overflow-y-auto">
          {pinned && (
            <>
              <MenuItem type="button" onClick={() => choose(pinned.id)}>
                <span className="inline-flex items-center gap-2">
                  {renderIcon?.(pinned)}
                  <span>
                    {renderLabel?.(pinned) ?? pinned.label}
                    {/* An unnamed sentinel's label already IS "Default", so the
                        usual marker would read "Default · Default" — say where
                        the behavior comes from instead. A named one ("Adaptive")
                        gets the standard marker. */}
                    <span className="option-default text-muted font-normal">
                      {m.model_picker_cli_configuration()}
                    </span>
                  </span>
                </span>
                {effectiveId === pinned.id && <Check size={14} />}
              </MenuItem>
              <div className="option-sep h-px m-1 bg-border-variant" />
            </>
          )}
          {visible.map((c, i) => (
            <MenuItem type="button" key={c.id} onClick={() => choose(c.id)}>
              <span className="flex min-w-0 items-center gap-2">
                {renderIcon?.(c)}
                <span className="flex min-w-0 flex-col items-start gap-0.5">
                  <span>
                    {renderLabel?.(c) ?? c.label}
                    {/* A concrete default renders inline, in ramp order, with just
                        the marker — it's one of the tiers, not a separate kind of
                        choice like the pinned sentinel above. */}
                    {!pinned && c.id === defaultId && (
                      <span className="option-default text-muted font-normal"> {m.model_picker_default()}</span>
                    )}
                  </span>
                  {c.description && (
                    <span className="max-w-68 text-sm font-normal leading-snug text-muted">
                      {c.description}
                    </span>
                  )}
                </span>
              </span>
              {effectiveId === c.id ? (
                <Check size={14} />
              ) : (
                numbered && <span className="option-num text-muted text-xs tabular-nums">{i + 1}</span>
              )}
            </MenuItem>
          ))}
          </div>
        </div>
      )}
      <EscapeScope enabled={open} onEscape={() => { setOpen(false); buttonRef.current?.focus(); }} />
    </div>
  );
}

const EMPTY_HARNESSES: Harness[] = [];
