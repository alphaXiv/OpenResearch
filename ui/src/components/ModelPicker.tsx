import {
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type ReactNode,
  type RefObject,
} from "react";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { useWorkspace } from "../workspace";
import { mutUpdateUiState } from "../queries/settings";
import { EscapeScope } from "./EscapeScope";
import { useQuery } from "@tanstack/react-query";
import { Menu } from "@base-ui/react/menu";
import { Popover } from "@base-ui/react/popover";
import { DirectionProvider } from "@base-ui/react/direction-provider";

import { m } from "../paraglide/messages.js";
import { Bot, Brain, Check, ChevronDown, ChevronRight, Handshake, ListChecks, Lock, LockOpen, Pencil, Plus, Ruler, Settings, ShieldCheck, Workflow, Zap, type LucideIcon } from "lucide-react";
import {
  modelLabel,
  reasoningFor,
  reconcileReasoning,
  reconcileServiceTier,
  REASONING_DEFAULT_ID,
  serviceTiersFor,
  type Harness,
  type HarnessId,
  type LocalModelConnection,
  type ModelVariant,
  type OpenCodeProvider,
  type OptionChoice,
  type AgentSelection,
  type Autonomy,
  DEFAULT_AUTONOMY,
} from "../api";
import type { NativeOption } from "../acp";
import { HarnessLogo, ModelMark } from "./HarnessLogo";

import { IconButton, MenuItem, SwitchIndicator, Tooltip } from "./ui";
import { cn } from "./ui/cn";

import { harnessKey, selectionHarness } from "../modelSelection";
import { defaultOffer, groupModelLines, NEW_FOR, sight, VARIANT_EFFORTS, variantOffer } from "../modelLines";
import { AgentIconTile } from "./AgentIcon";
import { lineName, ModelList, offerOn, type PickerLine } from "./ModelList";
import { pickerLines, type PickerAgent } from "./pickerAgents";

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

const HARNESS_LABELS: Record<HarnessId, string> = {
  "claude-code": "Claude Code",
  codex: "OpenAI Codex",
  opencode: "OpenCode",
  cursor: "Cursor",
  antigravity: "Google Antigravity",
};

export function harnessLabel(id: HarnessId, name?: string | null): string {
  return name || HARNESS_LABELS[id] || (id.startsWith("acp:") ? "ACP" : id);
}

/** The first agent with a model to start on, the fallback when nothing is picked.
 * Seeds mode/reasoning from that agent's advertised defaults. */
export function defaultSelection(harnesses: Harness[]): ModelSelection | null {
  const ready = harnesses.find((h) => h.defaultModel);
  const model = ready?.defaultModel;
  if (!ready || !model) return null;
  return {
    harness: ready.id,
    profile: ready.profile?.id ?? null,
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

const PILL =
  "composer-pill inline-flex h-7 min-w-0 shrink-0 items-center gap-1.5 rounded-md px-2 text-sm text-subtext whitespace-nowrap transition-[background,color] duration-150 ease-standard hover:bg-hover-muted hover:text-text data-[popup-open]:bg-hover-muted data-[popup-open]:text-text";

const MENU_POPUP = "overflow-y-auto rounded-lg border border-border bg-background p-1 shadow-dropdown outline-none";

const MENU_LABEL = "px-2 pt-1.5 pb-1 text-sm text-muted";

/** Effort tiers fill more of a 3×3 grid; ids the grid does not know stay empty. */
const EFFORT_DOTS: Record<string, { dots: number[]; tone: string }> = {
  minimal: { dots: [4], tone: "text-muted" },
  low: { dots: [4], tone: "text-accent-green" },
  medium: { dots: [3, 4, 5], tone: "text-accent-amber" },
  high: { dots: [1, 3, 4, 5, 7], tone: "text-accent-orange" },
  xhigh: { dots: [0, 2, 4, 6, 8], tone: "text-accent-red" },
  max: { dots: [0, 1, 2, 3, 4, 5, 6, 7, 8], tone: "text-accent-purple" },
};

/** Claude's `ultracode` is a session mode, `xhigh` plus dynamic workflows, so it toggles on top of the effort levels. */
const ULTRACODE = "ultracode";

function EffortIcon({ id }: { id: string | null | undefined }) {
  if (id === ULTRACODE) return <Workflow size={14} aria-hidden="true" className="shrink-0 text-accent-purple" />;

  const effort = (id && EFFORT_DOTS[id]) || (id?.startsWith("ultra") ? EFFORT_DOTS.max : null);
  return (
    <svg viewBox="0 0 14 14" className="size-3.5 shrink-0" aria-hidden="true">
      {Array.from({ length: 9 }, (_, dot) => (
        <rect
          key={dot}
          x={(dot % 3) * 5}
          y={Math.floor(dot / 3) * 5}
          width="4"
          height="4"
          rx="1"
          fill="currentColor"
          className={effort?.dots.includes(dot) ? effort.tone : "text-border"}
        />
      ))}
    </svg>
  );
}

/** Icons follow the shared policy behind each provider's permission id (`PermissionMode::from_id`). */
const PERMISSION_ICONS: Record<string, { icon: LucideIcon; tone: string }> = {
  ask: { icon: Lock, tone: "text-accent-amber" },
  manual: { icon: Lock, tone: "text-accent-amber" },
  default: { icon: Lock, tone: "text-accent-amber" },
  "accept-edits": { icon: Pencil, tone: "text-accent-orange" },
  acceptEdits: { icon: Pencil, tone: "text-accent-orange" },
  plan: { icon: ListChecks, tone: "text-accent-teal" },
  auto: { icon: ShieldCheck, tone: "text-accent-blue" },
  "approve-for-me": { icon: ShieldCheck, tone: "text-accent-blue" },
  "auto-approve": { icon: ShieldCheck, tone: "text-accent-blue" },
  bypass: { icon: LockOpen, tone: "text-accent-red" },
  bypassPermissions: { icon: LockOpen, tone: "text-accent-red" },
  "full-access": { icon: LockOpen, tone: "text-accent-red" },
};

function PermissionIcon({ id }: { id: string | null | undefined }) {
  const permission = id ? PERMISSION_ICONS[id] : undefined;
  const Icon = permission?.icon ?? ShieldCheck;
  return <Icon size={14} aria-hidden="true" className={cn("shrink-0", permission?.tone ?? "text-subtext")} />;
}

function DescribedRadioItem({ choice, icon, isDefault }: { choice: OptionChoice; icon?: ReactNode; isDefault: boolean }) {
  return (
    <Menu.RadioItem value={choice.id} closeOnClick label={choice.label} className={cn(SETTINGS_ROW, "items-start")}>
      <span className="flex min-w-0 items-start gap-2">
        {icon && <span className="flex h-5 items-center">{icon}</span>}
        <span className="flex min-w-0 flex-col items-start gap-0.5">
          <span>
            {choice.label}
            {isDefault && <span className="text-muted"> {m.model_picker_default()}</span>}
          </span>
          {choice.description && <span className="text-sm leading-snug text-muted">{choice.description}</span>}
        </span>
      </span>
      <Menu.RadioItemIndicator className="flex h-5 shrink-0 items-center">
        <Check size={14} aria-hidden="true" />
      </Menu.RadioItemIndicator>
    </Menu.RadioItem>
  );
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
  const { portalContainer } = useContext(WorkspaceDomContext);

  const [open, setOpen] = useState(false);

  return (
    <Menu.SubmenuRoot open={open} onOpenChange={setOpen}>
      <Menu.SubmenuTrigger delay={0} label={label} className={SETTINGS_ROW}>
        <span>{label}</span>
        <span className="ms-auto max-w-36 truncate text-muted">{detail}</span>
        <ChevronRight size={14} aria-hidden="true" className="shrink-0 text-muted rtl:rotate-180" />
      </Menu.SubmenuTrigger>
      <Menu.Portal container={portalContainer}>
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

/** Composer selector for the chat's harness and model. */
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
  disabled = false,
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
  disabled?: boolean;
  openRequest?: number;
  className?: string;
  sessionHarness?: Harness;
  nativeOptions?: NativeOption[];
  onSelectNativeOption?: (optionId: string, value: string) => void;
  onStartHarness?: (harness: Harness, model: string) => void;
}) {
  const { portalContainer } = useContext(WorkspaceDomContext);

  const workspace = useWorkspace();
  const { getHarnessesQuery, getOpenCodeProvidersQuery, getLocalModelsQuery, getUiStateQuery } = workspace.queries;

  const { data: catalog = EMPTY_HARNESSES, isPending: catalogLoading } = useQuery(getHarnessesQuery());
  const { data: providers = EMPTY_PROVIDERS } = useQuery(getOpenCodeProvidersQuery());
  const { data: endpoints = EMPTY_ENDPOINTS } = useQuery(getLocalModelsQuery());
  const { data: uiState } = useQuery(getUiStateQuery());
  const pickedHarness = value?.harness;
  const pickedProfile = value?.profile;
  const pickedModel = value?.model;
  const { harnesses, agents, offers, selectedHarness, selectedModel, selectedAgent, lines, selectedLine } = useMemo(
    () => pickerLines(
      { catalog, providers, endpoints, hiddenModels: uiState?.hiddenModels ?? EMPTY_IDS },
      pickedHarness ? { harness: pickedHarness, profile: pickedProfile, model: pickedModel ?? null } : null,
      sessionHarness,
    ),
    [catalog, providers, endpoints, uiState?.hiddenModels, pickedHarness, pickedProfile, pickedModel, sessionHarness],
  );
  const favorites = useMemo(() => new Set(uiState?.favoriteModels ?? []), [uiState]);
  const sightings = uiState?.modelSightings ?? null;
  const fresh = useMemo(() => new Set(Object.entries(sightings?.models ?? {}).filter(([, at]) => at > Date.now() - NEW_FOR).map(([key]) => key)), [sightings]);
  // Models that came out since the picker was last opened.
  const unseen = lines.filter((line) => !line.older && (sightings?.models[line.key] ?? 0) > (sightings?.openedAt ?? Infinity));
  const [menu, setMenu] = useState<"model" | "effort" | "permissions" | null>(null);
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
    if (disabled) return;
    setFilter("");
    setMenu("model");
    searchRef.current?.focus();
  }, [openRequest, disabled]);

  const selectedOffer = offerOn(selectedLine, selectedAgent, selectedModel);
  const lockedHarness = lockHarness && value ? value.profile ?? value.harness : undefined;

  const pick = (harness: Harness, model: string) => {
    if (disabled) return;
    const previous = selectionHarness([harness], value) ? value : null;

    onSelect({
      harness: harness.id,
      profile: harness.profile?.id ?? null,
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

  /** While the chat can't change agents, a model on another agent opens a new chat with it. */
  const run = (agent: PickerAgent, model: string) => {
    if (lockedHarness && harnessKey(agent.harness) !== lockedHarness) {
      close();
      onStartHarness?.(agent.harness, model);
    } else pick(agent.harness, model);
  };

  const pickLine = (line: PickerLine, agent: PickerAgent) => {
    const offer = (line === selectedLine && agent === selectedAgent && selectedOffer) || defaultOffer(line.offers.filter((offer) => offer.tile === agent));
    if (offer) run(agent, offer.model.id);
  };

  const toggleFavorite = (family: string) =>
    mutUpdateUiState.run(workspace, { favoriteModels: favorites.has(family) ? [...favorites].filter((item) => item !== family) : [...favorites, family] });

  // Every model is seen before it's hidden, so turning one back on later doesn't make it new. A write that
  // failed isn't retried until the picker mounts again, and a server too old to keep sightings gets none.
  const sighting = useRef(false);
  useEffect(() => {
    if (!uiState || !("modelSightings" in uiState) || sighting.current) return;
    const all = groupModelLines(offers);
    const next = sight(uiState.modelSightings, all, Date.now());
    if (next === uiState.modelSightings || !all.length) return;
    sighting.current = true;
    mutUpdateUiState.runWithOptions(workspace, { modelSightings: next }, { onSettled: ({ status }) => { sighting.current = status !== "success"; } });
  }, [uiState, offers]);

  // A saved model keeps its name while its agent's models load; once they have, only a listed one does.
  const loading = catalogLoading || Boolean(selectedHarness?.catalogPending);
  const label = selectedLine
    ? lineName(selectedLine)
    : loading
      ? value?.model ? modelLabel(value.model) : m.model_picker_model()
      : m.model_picker_choose_model();
  // A model only one agent offers names no agent.
  const agentShown = selectedAgent && new Set(selectedLine?.offers.map((offer) => offer.tile)).size > 1 ? selectedAgent : undefined;
  const hint = agentShown ? `${label} — ${agentShown.name}` : label;

  // Cursor and Antigravity list each effort, speed and thinking mode as a model, and Claude
  // Code its long context: the Effort menu moves between the siblings of the picked one.
  const siblings = selectedLine?.offers.filter((offer) => offer.tile === selectedAgent) ?? [];
  const variant = selectedOffer?.model.parsed.variant;
  const varies = (field: keyof ModelVariant) => new Set(siblings.map((offer) => offer.model.parsed.variant[field])).size > 1;
  const variantEfforts = varies("effort")
    ? [...new Set(siblings.map((offer) => offer.model.parsed.variant.effort))].sort((a, b) => VARIANT_EFFORTS.indexOf(a as never) - VARIANT_EFFORTS.indexOf(b as never))
    : [];
  const variantContext = varies("context") ? siblings.find((offer) => offer.model.parsed.variant.context)?.model.parsed.variant.context : undefined;
  const pickVariant = (change: Partial<ModelVariant>) => {
    const offer = variant && variantOffer(siblings, variant, change);
    if (!offer || !value || !selectedHarness) return;
    onSelect({ ...value, model: offer.model.id, reasoningLevel: reconcileReasoning(selectedHarness, offer.model.id, value.reasoningLevel) });
  };
  const variantSwitches = Boolean(variant && (varies("fast") || varies("thinking") || variantContext));
  const hasVariants = Boolean(variant && (variantEfforts.length || variantSwitches));

  const effectiveReasoningId = value?.reasoningLevel ?? defaultReasoningId ?? reasoningChoices[0]?.id;
  const reasoningLabel = reasoningChoices.find((choice) => choice.id === effectiveReasoningId)?.label;
  const ultracodeChoice = reasoningChoices.find((choice) => choice.id === ULTRACODE);
  const effortChoices = reasoningChoices.filter((choice) => choice.id !== ULTRACODE);
  const ultracode = effectiveReasoningId === ULTRACODE;
  const effectivePermissionId = value?.permissionMode ?? defaultPermissionId ?? permissionChoices[0]?.id;
  const permissionLabel = permissionChoices.find((choice) => choice.id === effectivePermissionId)?.label;
  const effectiveServiceTier = reconcileServiceTier(selectedHarness, value?.model, value?.serviceTier);
  const fastChoice = serviceTiersFor(selectedHarness, value?.model).find((choice) => choice.id === "priority");
  const effortLabel = variant && variantEfforts.length ? effortName(variant.effort) : reasoningLabel;
  const fastOn = effectiveServiceTier === "priority" || Boolean(variant?.fast);
  const autonomyChoices: (OptionChoice & { id: Autonomy; icon: ReactNode })[] = [
    {
      id: "agentic",
      label: m.model_picker_autonomy_agentic(),
      description: m.model_picker_autonomy_agentic_description(),
      icon: <Bot size={14} aria-hidden="true" className="shrink-0 text-accent-purple" />,
    },
    {
      id: "copilot",
      label: m.model_picker_autonomy_copilot(),
      description: m.model_picker_autonomy_copilot_description(),
      icon: <Handshake size={14} aria-hidden="true" className="shrink-0 text-accent-green" />,
    },
  ];
  const autonomyLabel = autonomyChoices.find((choice) => choice.id === autonomy)?.label;
  const moveFocus = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const items = Array.from(
      event.currentTarget.querySelectorAll<HTMLElement>("input, .model-menu-list [data-nav]:not([disabled])"),
    );
    if (!items.length) return;

    event.preventDefault();
    // From one of a row's agents, Up and Down still move by row.
    const active = document.activeElement?.closest("[data-row]")?.querySelector("[data-nav]") ?? document.activeElement;
    const index = items.findIndex((item) => item === active);
    // Down from the search enters the list where it's scrolled to, below the sticky section header.
    const list = event.currentTarget.querySelector(".model-menu-list");
    const top = list ? list.getBoundingClientRect().top + parseFloat(getComputedStyle(list).scrollPaddingTop) : 0;
    const shown = active === searchRef.current && event.key === "ArrowDown" ? items.find((item) => item !== active && item.getBoundingClientRect().top >= top) : undefined;
    (shown ?? items[(index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length])?.focus();
  };

  return (
    <DirectionProvider direction={document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
      <div
        className="model-picker relative inline-flex min-w-0 items-center gap-0.5"
        data-onboarding="model-picker"
      >
        <Popover.Root
          open={!disabled && menu === "model"}
          onOpenChange={(open) => {
            setFilter("");
            setMenu((current) => (open ? "model" : current === "model" ? null : current));
            if (open && sightings && unseen.length) mutUpdateUiState.run(workspace, { modelSightings: { ...sightings, openedAt: Date.now() } });
          }}
        >
          <Tooltip interactive content={hint} className="min-w-0 max-w-full rounded-md">
            <Popover.Trigger
              ref={modelTriggerRef}
              type="button"
              disabled={disabled}
              className={cn(PILL, "max-w-full", className)}
              aria-label={[m.a11y_chat_model({ label: hint }), unseen.length ? m.model_picker_new_model() : null].filter(Boolean).join(", ")}
              onKeyDown={(event) => {
                if (event.key === "ArrowDown") {
                  event.preventDefault();
                  setFilter("");
                  setMenu("model");
                  searchRef.current?.focus();
                }
              }}
            >
              {agentShown ? (
                <AgentIconTile icon={agentShown.icon} brand={agentShown.brand} size="xs" />
              ) : selectedAgent ? null : value?.harness ? (
                <HarnessLogo harness={value.harness} logo={selectedHarness?.logo} size={14} />
              ) : null}
              {selectedOffer && <ModelMark model={selectedOffer.model} harness={selectedOffer.tile.harness.id} size={14} />}
              <span className="model-picker-label min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">
                {label}
              </span>
              {unseen.length > 0 && <span className="shrink-0 rounded-sm bg-accent-blue-subtle px-1 text-xs leading-4 text-accent-blue-strong">{m.model_picker_new_model()}</span>}
              <ChevronDown size={14} className="shrink-0 text-muted" />
            </Popover.Trigger>
          </Tooltip>
          <Popover.Portal container={portalContainer}>
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
                  width: "min(440px, calc(100vw - 16px))",
                  maxHeight: "min(280px, var(--available-height))",
                }}
              >
                <div className="flex shrink-0 items-center border-b border-border-variant pe-1.5">
                  <input
                    ref={searchRef}
                    type="text"
                    aria-label={m.model_picker_search_models()}
                    placeholder={m.model_picker_search_models()}
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
                <ModelList
                  lines={lines}
                  agents={agents}
                  filter={filter}
                  onClearFilter={() => setFilter("")}
                  favorites={favorites}
                  fresh={fresh}
                  order={uiState?.modelSectionOrder ?? EMPTY_IDS}
                  collapsed={uiState?.collapsedModelSections ?? EMPTY_IDS}
                  onCollapse={(collapsedModelSections) => mutUpdateUiState.run(workspace, { collapsedModelSections })}
                  selectedLine={selectedLine}
                  selectedAgent={selectedAgent}
                  lockedHarness={lockedHarness}
                  onPick={pickLine}
                  onToggleFavorite={toggleFavorite}
                >
                  {harnesses.length === 0 && (
                    <div className={MODEL_MORE_CLASS_NAME}>{m.model_picker_detecting_harnesses()}</div>
                  )}
                </ModelList>
              </Popover.Popup>
            </Popover.Positioner>
          </Popover.Portal>
        </Popover.Root>
        {(reasoningChoices.length > 0 || nativeOptions.length > 0 || fastChoice || hasVariants) && (
          <Menu.Root
            open={!disabled && menu === "effort"}
            modal={false}
            onOpenChange={(open) => setMenu((current) => (open ? "effort" : current === "effort" ? null : current))}
          >
            <Menu.Trigger
              disabled={disabled}
              className={PILL}
              aria-label={[
                effortLabel ? `${m.model_picker_effort()}: ${effortLabel}` : m.model_picker_effort(),
                fastOn ? m.model_picker_fast_mode() : null,
              ].filter(Boolean).join(" · ")}
            >
              {fastOn && <Zap size={14} fill="currentColor" aria-hidden="true" className="shrink-0 text-accent-amber" />}
              {effortLabel ? (
                <>
                  <EffortIcon id={variantEfforts.length ? variant?.effort : effectiveReasoningId} />
                  <span>{effortLabel}</span>
                </>
              ) : (
                <Settings size={14} aria-hidden="true" />
              )}
              <ChevronDown size={14} className="shrink-0 text-muted" />
            </Menu.Trigger>
            <Menu.Portal container={portalContainer}>
              <Menu.Positioner className="z-50" side="top" align="end" sideOffset={8} collisionPadding={8} positionMethod="fixed">
                <Menu.Popup aria-label={m.model_picker_effort()} className={MENU_POPUP} style={{ width: "min(240px, calc(100vw - 16px))", maxHeight: "min(400px, var(--available-height))" }}>
                  {effortChoices.length > 0 && (
                    <Menu.Group>
                      <Menu.GroupLabel className={MENU_LABEL}>{m.model_picker_effort()}</Menu.GroupLabel>
                      <Menu.RadioGroup
                        value={ultracode ? "xhigh" : effectiveReasoningId}
                        onValueChange={(id) => {
                          if (typeof id === "string") onSelectReasoning?.(id);
                        }}
                      >
                        {effortChoices.map((choice) => (
                          <Menu.RadioItem key={choice.id} value={choice.id} closeOnClick label={choice.label} className={SETTINGS_ROW}>
                            <span className="inline-flex items-center gap-2">
                              <EffortIcon id={choice.id} />
                              {choice.label}
                              {choice.id === defaultReasoningId && choice.id !== REASONING_DEFAULT_ID && <span className="text-muted">{m.model_picker_default()}</span>}
                            </span>
                            <Menu.RadioItemIndicator>
                              <Check size={14} />
                            </Menu.RadioItemIndicator>
                          </Menu.RadioItem>
                        ))}
                      </Menu.RadioGroup>
                    </Menu.Group>
                  )}
                  {variant && variantEfforts.length > 0 && (
                    <Menu.Group>
                      <Menu.GroupLabel className={MENU_LABEL}>{m.model_picker_effort()}</Menu.GroupLabel>
                      <Menu.RadioGroup
                        value={variant.effort ?? ""}
                        onValueChange={(id) => {
                          if (typeof id === "string") pickVariant({ effort: id || null });
                        }}
                      >
                        {variantEfforts.map((effort) => (
                          <Menu.RadioItem key={effort ?? ""} value={effort ?? ""} closeOnClick label={effortName(effort)} className={SETTINGS_ROW}>
                            <span className="inline-flex items-center gap-2">
                              <EffortIcon id={effort} />
                              {effortName(effort)}
                            </span>
                            <Menu.RadioItemIndicator>
                              <Check size={14} />
                            </Menu.RadioItemIndicator>
                          </Menu.RadioItem>
                        ))}
                      </Menu.RadioGroup>
                    </Menu.Group>
                  )}
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
                  {(ultracodeChoice || fastChoice || variantSwitches) && (effortChoices.length > 0 || variantEfforts.length > 0 || nativeOptions.length > 0) && (
                    <Menu.Separator className="m-1 h-px bg-border-variant" />
                  )}
                  {ultracodeChoice && (
                    <Tooltip interactive side="start" content={m.model_picker_ultracode_description()} className="flex w-full rounded-sm">
                      <Menu.CheckboxItem
                        className={SETTINGS_ROW}
                        checked={ultracode}
                        onCheckedChange={(checked) => onSelectReasoning?.(checked ? ULTRACODE : "xhigh")}
                      >
                        <span className="inline-flex items-center gap-2">
                          <EffortIcon id={ULTRACODE} />
                          {ultracodeChoice.label}
                        </span>
                        <SwitchIndicator checked={ultracode} aria-hidden="true" />
                      </Menu.CheckboxItem>
                    </Tooltip>
                  )}
                  {fastChoice && (
                    <Menu.CheckboxItem
                      className={SETTINGS_ROW}
                      checked={effectiveServiceTier === fastChoice.id}
                      onCheckedChange={(checked) => {
                        if (value) onSelect({ ...value, serviceTier: checked ? fastChoice.id : "default" });
                      }}
                    >
                      <span className="inline-flex items-center gap-2">
                        <Zap size={14} aria-hidden="true" className="text-accent-amber" />
                        {m.model_picker_fast_mode()}
                      </span>
                      <SwitchIndicator checked={effectiveServiceTier === fastChoice.id} aria-hidden="true" />
                    </Menu.CheckboxItem>
                  )}
                  {variant && varies("fast") && (
                    <Menu.CheckboxItem className={SETTINGS_ROW} checked={variant.fast} onCheckedChange={(checked) => pickVariant({ fast: checked })}>
                      <span className="inline-flex items-center gap-2">
                        <Zap size={14} aria-hidden="true" className="text-accent-amber" />
                        {m.model_picker_fast_mode()}
                      </span>
                      <SwitchIndicator checked={variant.fast} aria-hidden="true" />
                    </Menu.CheckboxItem>
                  )}
                  {variant && varies("thinking") && (
                    <Menu.CheckboxItem className={SETTINGS_ROW} checked={variant.thinking} onCheckedChange={(checked) => pickVariant({ thinking: checked })}>
                      <span className="inline-flex items-center gap-2">
                        <Brain size={14} aria-hidden="true" className="text-accent-purple" />
                        {m.model_picker_thinking()}
                      </span>
                      <SwitchIndicator checked={variant.thinking} aria-hidden="true" />
                    </Menu.CheckboxItem>
                  )}
                  {variant && variantContext && (
                    <Menu.CheckboxItem className={SETTINGS_ROW} checked={variant.context !== null} onCheckedChange={(checked) => pickVariant({ context: checked ? variantContext : null })}>
                      <span className="inline-flex items-center gap-2">
                        <Ruler size={14} aria-hidden="true" className="text-accent-teal" />
                        {m.model_picker_context_size({ size: variantContext.toUpperCase() })}
                      </span>
                      <SwitchIndicator checked={variant.context !== null} aria-hidden="true" />
                    </Menu.CheckboxItem>
                  )}
                </Menu.Popup>
              </Menu.Positioner>
            </Menu.Portal>
          </Menu.Root>
        )}
        {(permissionChoices.length > 0 || onSelectAutonomy) && (
          <Menu.Root
            open={!disabled && menu === "permissions"}
            modal={false}
            onOpenChange={(open) => setMenu((current) => (open ? "permissions" : current === "permissions" ? null : current))}
          >
            <Tooltip interactive content={[permissionLabel, autonomyLabel].filter(Boolean).join(" · ")} className="rounded-md">
              <Menu.Trigger
                disabled={disabled}
                className={PILL}
                aria-label={`${m.model_picker_permissions()}: ${[permissionLabel, autonomyLabel].filter(Boolean).join(", ")}`}
              >
                <PermissionIcon id={effectivePermissionId} />
                <ChevronDown size={14} className="shrink-0 text-muted" />
              </Menu.Trigger>
            </Tooltip>
            <Menu.Portal container={portalContainer}>
              <Menu.Positioner className="z-50" side="top" align="end" sideOffset={8} collisionPadding={8} positionMethod="fixed">
                <Menu.Popup
                  aria-label={m.model_picker_permissions()}
                  className={MENU_POPUP}
                  style={{ width: "min(320px, calc(100vw - 16px))", maxHeight: "min(560px, var(--available-height))" }}
                >
                  {permissionChoices.length > 0 && (
                    <Menu.Group>
                      <Menu.GroupLabel className={MENU_LABEL}>{m.model_picker_permissions()}</Menu.GroupLabel>
                      <Menu.RadioGroup
                        value={effectivePermissionId}
                        onValueChange={(id) => {
                          if (typeof id === "string") onSelectPermission?.(id);
                        }}
                      >
                        {permissionChoices.map((choice) => (
                          <DescribedRadioItem key={choice.id} choice={choice} icon={<PermissionIcon id={choice.id} />} isDefault={choice.id === defaultPermissionId} />
                        ))}
                      </Menu.RadioGroup>
                    </Menu.Group>
                  )}
                  {permissionChoices.length > 0 && onSelectAutonomy && <Menu.Separator className="m-1 h-px bg-border-variant" />}
                  {onSelectAutonomy && (
                    <Menu.Group>
                      <Menu.GroupLabel className={MENU_LABEL}>{m.model_picker_autonomy()}</Menu.GroupLabel>
                      <Menu.RadioGroup
                        value={autonomy ?? DEFAULT_AUTONOMY}
                        onValueChange={(id) => {
                          const choice = autonomyChoices.find((entry) => entry.id === id);
                          if (choice) onSelectAutonomy(choice.id);
                        }}
                      >
                        {autonomyChoices.map((choice) => (
                          <DescribedRadioItem key={choice.id} choice={choice} icon={choice.icon} isDefault={choice.id === DEFAULT_AUTONOMY} />
                        ))}
                      </Menu.RadioGroup>
                    </Menu.Group>
                  )}
                </Menu.Popup>
              </Menu.Positioner>
            </Menu.Portal>
          </Menu.Root>
        )}
      </div>
      <EscapeScope enabled={menu !== null} onEscape={close} />
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
const EMPTY_PROVIDERS: OpenCodeProvider[] = [];
const EMPTY_ENDPOINTS: LocalModelConnection[] = [];
const EMPTY_IDS: string[] = [];

const EFFORT_NAMES: Record<string, () => string> = {
  none: m.model_picker_effort_none,
  minimal: m.model_picker_effort_minimal,
  low: m.model_picker_effort_low,
  medium: m.model_picker_effort_medium,
  high: m.model_picker_effort_high,
  xhigh: m.model_picker_effort_xhigh,
  max: m.model_picker_effort_max,
};

/** A variant's effort; Cursor's plain variant has none of its own. */
function effortName(effort: string | null) {
  return effort ? (EFFORT_NAMES[effort]?.() ?? effort) : m.model_picker_effort_default();
}
