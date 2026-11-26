import {
  useContext,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type CSSProperties,
  type ReactElement,
  type ReactNode,
  type RefObject,
} from "react";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { useRuntime } from "../RemoteRuntime";
import { useWorkspace } from "../workspace";
import { mutUpdateUiState } from "../queries/settings";
import { EscapeScope } from "./EscapeScope";
import { useQuery } from "@tanstack/react-query";
import { Menu } from "@base-ui/react/menu";
import { Popover } from "@base-ui/react/popover";
import { DirectionProvider } from "@base-ui/react/direction-provider";

import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import { Brain, Check, ChevronDown, ChevronRight, ListChecks, Lock, LockOpen, Minus, Pencil, Plus, Rocket, Ruler, Settings, ShieldCheck, Turtle, Workflow, Zap, type LucideIcon } from "lucide-react";
import {
  modelLabel,
  reasoningFor,
  reconcileReasoning,
  reconcileServiceTier,
  REASONING_DEFAULT_ID,
  SERVICE_TIER_DEFAULT_ID,
  serviceTiersFor,
  type Harness,
  type HarnessId,
  type LocalModelConnection,
  type ModelNudge,
  type ModelVariant,
  type OpenCodeProvider,
  type OptionChoice,
  type AgentSelection,
} from "../api";
import type { NativeOption } from "../acp";
import { HarnessLogo, LABS, ModelGlyph, ModelMark } from "./HarnessLogo";

import { Button, IconButton, MENU_LABEL, MENU_POPUP, MENU_SEPARATOR, MenuItem, SwitchIndicator, Tooltip } from "./ui";
import { cn } from "./ui/cn";

import { harnessDisplayName, harnessKey, selectionHarness } from "../modelSelection";
import { defaultOffer, localDay, newLines, VARIANT_EFFORTS, variantOffer } from "../modelLines";
import { AgentIconTile } from "./AgentIcon";
import { feedLine, lineLab, lineName, lineSection, ModelList, offerOn, type PickedFrom, type PickerLine } from "./ModelList";
import { harnessMark, listedSignedOut, pickerLines, type PickerAgent } from "./pickerAgents";
import { useAgentSignIn } from "./HarnessSetupDialog";

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
  "auto-approve": { icon: LockOpen, tone: "text-accent-red" },
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

/** Speed tiers slowest first, as the Speed control offers them; null is the standard tier. */
const SPEEDS = ["flex", null, "fast", "ultrafast"] as const;
const SPEED_ICONS: Record<string, LucideIcon> = { flex: Turtle, fast: Zap, ultrafast: Rocket };

function speedName(speed: string | null) {
  return speed === "fast" ? m.model_picker_speed_fast()
    : speed === "ultrafast" ? m.model_picker_speed_ultrafast()
      : speed === "flex" ? m.model_picker_speed_flex()
        : speed ?? m.service_tier_standard();
}

function SpeedIcon({ speed }: { speed: string | null }) {
  const Icon = speed ? SPEED_ICONS[speed] ?? Zap : Minus;
  return <Icon size={14} aria-hidden="true" className={cn("shrink-0", speed && "text-accent-amber")} />;
}

const SPEED_SEGMENT =
  "inline-flex h-6 w-7 cursor-default items-center justify-center rounded-sm text-subtext outline-none transition-[background,color] duration-120 ease-standard data-[highlighted]:text-text data-[checked]:bg-background data-[checked]:text-text data-[checked]:shadow-segment focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus";

const SETTINGS_ROW =
  // Inset, so a scrolling menu never clips the focus ring.
  "flex min-h-8 w-full items-center justify-between gap-2 rounded-sm px-2 py-1.5 text-start text-sm text-text outline-none transition-[background,color] duration-120 ease-standard data-[highlighted]:bg-surface data-[popup-open]:bg-surface focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:-outline-offset-2";

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
  disabled = false,
  openRequest = 0,
  className,
  sessionHarness,
  nativeOptions = [],
  onSelectNativeOption,
  canNudge = false,
  only,
  trigger,
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
  disabled?: boolean;
  openRequest?: number;
  className?: string;
  sessionHarness?: Harness;
  nativeOptions?: NativeOption[];
  onSelectNativeOption?: (optionId: string, value: string) => void;
  /** Whether this composer shows the model feed's nudge: the open chat's does. */
  canNudge?: boolean;
  /** Lists only the models this accepts, and no agent to sign in to. */
  only?: (harness: Harness, model: string) => boolean;
  /** Stands in for the pill that names the picked model. */
  trigger?: ReactElement;
}) {
  const { portalContainer } = useContext(WorkspaceDomContext);

  const workspace = useWorkspace();
  const runtime = useRuntime();
  const { getHarnessesQuery, getOpenCodeProvidersQuery, getLocalModelsQuery, getUiStateQuery, getModelFeedQuery } = workspace.queries;

  const { data: catalog = EMPTY_HARNESSES, isPending: catalogLoading } = useQuery(getHarnessesQuery());
  const { data: providers = EMPTY_PROVIDERS } = useQuery(getOpenCodeProvidersQuery());
  const { data: endpoints = EMPTY_ENDPOINTS } = useQuery(getLocalModelsQuery());
  const { data: uiState } = useQuery(getUiStateQuery());
  const { data: feed } = useQuery(getModelFeedQuery());
  const pickedHarness = value?.harness;
  const pickedProfile = value?.profile;
  const pickedModel = value?.model;
  const { harnesses, agents, selectedHarness, selectedModel, selectedAgent, lines, selectedLine } = useMemo(() => {
    const listed = pickerLines(
      { catalog, providers, endpoints, hiddenModels: uiState?.hiddenModels ?? EMPTY_IDS },
      pickedHarness ? { harness: pickedHarness, profile: pickedProfile, model: pickedModel ?? null } : null,
      sessionHarness,
    );
    if (!only) return listed;

    const lines = listed.lines.flatMap((line) => {
      const offers = line.offers.filter((offer) => only(offer.tile.harness, offer.model.id));
      return offers.length ? [{ ...line, offers }] : [];
    });
    return { ...listed, lines, selectedLine: lines.find((line) => line.key === listed.selectedLine?.key) };
  }, [catalog, providers, endpoints, uiState?.hiddenModels, pickedHarness, pickedProfile, pickedModel, sessionHarness, only]);
  const favorites = useMemo(() => new Set(uiState?.favoriteModels ?? []), [uiState]);
  const releases = feed?.releases ?? NO_RELEASES;
  // Not memoized: a model stops being new as time passes, with nothing else changing.
  const released = newLines(lines, releases, Date.now());
  const [menu, setMenu] = useState<"model" | "effort" | "permissions" | null>(null);
  const [filter, setFilter] = useState("");
  const signIn = useAgentSignIn();
  // An agent signed out of research it ran keeps a row here, so signing in again is one click away.
  const signedOut = catalog.filter((h) => !only && listedSignedOut(h) && !h.off).flatMap((h) => {
    const name = h.profile?.name ?? harnessDisplayName(h);
    const open = signIn(h);
    return open && name.toLowerCase().includes(filter.trim().toLowerCase()) ? [{ h, name, open }] : [];
  });
  const modelTriggerRef = useRef<HTMLButtonElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const handledOpenRequest = useRef(openRequest);

  const close = () => {
    setMenu(null);
    setFilter("");
    setPointAt(undefined);
    modelTriggerRef.current?.focus();
  };

  // The open chat's model button points at one model at a time, once each: the newest release an agent offers, then
  // the feed's nudge toward a free model while free models are on. After one is dismissed, the next waits for a reload.
  // None in a development build.
  const nudge = feed?.nudge;
  const nudgeLine = nudge && uiState?.opencodeFreeModels ? feedLine(lines, nudge.model) : undefined;
  const nudgeText = nudge && nudgeLine && nudgeMessage(nudge);
  const installed = uiState?.createdAt ? new Date(uiState.createdAt) : undefined;
  const installedDayBefore = installed && new Date(installed.getFullYear(), installed.getMonth(), installed.getDate() - 1).getTime();
  const release = released.find(
    (line) =>
      !line.offers[0].model.free &&
      FRONTIER_LABS.includes(lineLab(line) as never) &&
      // A model that came out before the day before this copy was installed isn't news to its user.
      !(installedDayBefore && localDay(releases[line.key]).getTime() < installedDayBefore),
  );
  const callouts: Callout[] = [
    ...(release ? [{
      kind: "release" as const,
      id: `release:${release.key}`.slice(0, 64),
      line: release,
      text: releaseText(release, releases[release.key]),
      colors: releaseColors(release),
      mark: <ModelMark model={release.offers[0].model} harness={release.offers[0].tile.harness.id} size={24} />,
    }] : []),
    ...(nudge && nudgeLine && nudgeText ? [{
      kind: "free" as const,
      id: nudge.id,
      line: nudgeLine,
      text: nudgeText,
      colors: nudge.colors,
      mark: <span className="contents" style={{ color: nudge.colors.icon }}><ModelGlyph model={nudgeLine.offers[0].model} size={24} /></span>,
    }] : []),
  ];
  const seenNudges = uiState?.seenNudges;
  const callout = canNudge && runtime.buildChannel !== "development" && !disabled && seenNudges && !calledOut.has(workspace) ? callouts.find(({ id, line }) => !seenNudges.includes(id) && line !== selectedLine) : undefined;
  const leaving = useRef<number>(undefined);
  useEffect(() => () => clearTimeout(leaving.current), []);

  /** Records the callout as seen, keeping the newest ids since only a recent one returns; `expand` also opens the picker section it points into. */
  const dismissCallout = (expand = false) => {
    clearTimeout(leaving.current);
    if (!callout || !seenNudges) return;
    calledOut.add(workspace);
    reportCallout(callout, expand ? "opened" : "dismissed");
    const collapsed = uiState?.collapsedModelSections ?? EMPTY_IDS;
    const section = lineSection(callout.line);
    mutUpdateUiState.run(workspace, {
      seenNudges: [...seenNudges, callout.id].slice(-20),
      ...(expand && collapsed.includes(section) && { collapsedModelSections: collapsed.filter((id) => id !== section) }),
    });
  };
  const reportCallout = ({ kind, line }: Callout, action: "shown" | "opened" | "dismissed") => {
    const [offer] = line.offers;
    workspace.api.captureUiEvent({ name: "model_callout", kind, action, model: offer.model.id, harness: offer.tile.harness.id });
  };
  const showing = callout && menu === null ? callout : undefined;
  useEffect(() => {
    if (!showing || shownCallouts.has(showing.id)) return;
    shownCallouts.add(showing.id);
    reportCallout(showing, "shown");
  }, [showing?.id]);

  // Moving from the button to the callout crosses a gap, so leaving either waits before dismissing.
  const calloutHover = { onMouseEnter: () => clearTimeout(leaving.current), onMouseLeave: () => { leaving.current = window.setTimeout(dismissCallout, 200); } };

  const openModels = () => {
    setFilter("");
    setMenu("model");
    searchRef.current?.focus();
  };
  const openOnArrowDown = (event: React.KeyboardEvent) => {
    if (event.key !== "ArrowDown") return;
    event.preventDefault();
    dismissCallout();
    openModels();
  };

  // The picker opens on the model a callout pointed at, else on the newest release; typing still searches.
  const [pointAt, setPointAt] = useState<string>();
  const landOn = () => {
    const key = pointAt ?? released[0]?.key;
    const row = key && searchRef.current?.closest(".model-menu")?.querySelector<HTMLElement>(`[data-row="${CSS.escape(key)}"] [data-nav]`);
    return row || searchRef.current;
  };

  useEffect(() => {
    if (openRequest === handledOpenRequest.current) return;
    handledOpenRequest.current = openRequest;
    if (disabled) return;
    dismissCallout();
    openModels();
  }, [openRequest, disabled]);

  const selectedOffer = offerOn(selectedLine, selectedAgent, selectedModel);

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

  const pickLine = (line: PickerLine, agent: PickerAgent, from: PickedFrom) => {
    const offer = (line === selectedLine && agent === selectedAgent && selectedOffer) || defaultOffer(line.offers.filter((offer) => offer.tile === agent));
    if (!offer) return;
    workspace.api.captureUiEvent({
      name: "model_picked",
      section: from,
      model: offer.model.id,
      harness: agent.harness.id,
      new: released.includes(line),
      free: Boolean(offer.model.free),
    });
    pick(agent.harness, offer.model.id);
  };

  const toggleFavorite = (family: string) =>
    mutUpdateUiState.run(workspace, { favoriteModels: favorites.has(family) ? [...favorites].filter((item) => item !== family) : [...favorites, family] });

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

  // Catalogs list speed and thinking tiers as models, Cursor and Antigravity each effort too, and Claude
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
  const variantSpeeds = varies("speed") ? SPEEDS.filter((speed) => siblings.some((offer) => offer.model.parsed.variant.speed === speed)) : [];
  const variantSwitches = Boolean(variant && (variantSpeeds.length || varies("thinking") || variantContext));
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
  // Codex's Fast is a service tier of the same model; elsewhere each speed is a sibling model.
  const speeds: (string | null)[] = variantSpeeds.length ? variantSpeeds : fastChoice ? [null, "fast"] : [];
  const speed = variant?.speed ?? (fastChoice && effectiveServiceTier === fastChoice.id ? "fast" : null);
  const pickSpeed = (next: string | null) => {
    if (variantSpeeds.length) pickVariant({ speed: next });
    else if (value && fastChoice) onSelect({ ...value, serviceTier: next ? fastChoice.id : SERVICE_TIER_DEFAULT_ID });
  };
  // One tier beside the standard one is a switch; more are a choice.
  const switchTier = speeds.length === 2 && speeds.includes(null) ? speeds.find((tier) => tier !== null) : undefined;
  const speedLabel = (tier: string) => (tier === "fast" ? m.model_picker_fast_mode() : speedName(tier));
  const permissionSummary = permissionLabel ? `${m.model_picker_permissions()}: ${permissionLabel}` : m.model_picker_permissions();
  const moveFocus = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;

    // Typing anywhere in the picker continues the search.
    const typed = event.key.length === 1 && event.key !== " " ? filter + event.key : event.key === "Backspace" ? filter.slice(0, -1) : undefined;
    if (typed !== undefined && event.target !== searchRef.current && !event.ctrlKey && !event.metaKey && !event.altKey) {
      event.preventDefault();
      setFilter(typed);
      searchRef.current?.focus();
      return;
    }

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
        data-onboarding={trigger ? undefined : "model-picker"}
      >
        <Popover.Root
          open={!disabled && menu === "model"}
          onOpenChange={(open) => {
            setFilter("");
            setMenu((current) => (open ? "model" : current === "model" ? null : current));
            if (!open) setPointAt(undefined);
            if (open) dismissCallout();
          }}
        >
          {trigger ? <Popover.Trigger ref={modelTriggerRef} disabled={disabled} render={trigger} onKeyDown={openOnArrowDown} /> : <Tooltip interactive disabled={Boolean(callout)} content={hint} className="min-w-0 max-w-full rounded-md">
            <Popover.Trigger
              ref={modelTriggerRef}
              type="button"
              disabled={disabled}
              className={cn(PILL, "max-w-full", className)}
              aria-label={m.a11y_chat_model({ label: hint })}
              aria-description={callout ? `${lineName(callout.line)}, ${callout.text}` : undefined}
              {...(callout && calloutHover)}
              onKeyDown={openOnArrowDown}
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
              <ChevronDown size={14} className="shrink-0 text-muted" />
            </Popover.Trigger>
          </Tooltip>}
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
                initialFocus={landOn}
                onKeyDown={moveFocus}
                aria-label={m.model_picker_model()}
                className="model-menu group/picker flex flex-col overflow-hidden rounded-lg border border-border bg-background shadow-dropdown outline-none"
                style={{
                  width: "min(440px, calc(100vw - 16px))",
                  maxHeight: "min(340px, var(--available-height))",
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
                    onKeyDown={(event) => {
                      if (event.key !== "Enter" || event.nativeEvent.isComposing || event.keyCode === 229 || !filter.trim()) return;
                      event.preventDefault();
                      event.currentTarget.closest(".model-menu")?.querySelector<HTMLElement>("[data-row] [data-nav]")?.click();
                    }}
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
                  released={released}
                  pointedAt={pointAt}
                  featured={feed?.featured.map(({ model }) => model) ?? EMPTY_IDS}
                  order={uiState?.modelSectionOrder ?? EMPTY_IDS}
                  collapsed={uiState?.collapsedModelSections ?? EMPTY_IDS}
                  onCollapse={(collapsedModelSections) => mutUpdateUiState.run(workspace, { collapsedModelSections })}
                  selectedLine={selectedLine}
                  selectedAgent={selectedAgent}
                  onPick={pickLine}
                  onToggleFavorite={toggleFavorite}
                >
                  {harnesses.length === 0 && (
                    <div className={MODEL_MORE_CLASS_NAME}>{m.model_picker_detecting_harnesses()}</div>
                  )}
                  {signedOut.map(({ h, name, open }) => {
                    const mark = harnessMark(h);
                    return (
                      <div key={harnessKey(h)} className="flex items-center gap-2 px-2 py-1.5 text-sm">
                        <AgentIconTile icon={mark.icon} brand={mark.brand} size="xs" />
                        <span className="min-w-0 flex-1 truncate">
                          {name}
                          <span className="text-muted"> · {m.settings_page_not_signed_in()}</span>
                        </span>
                        <Button type="button" size="small" data-nav onClick={() => { close(); open(); }}>
                          {m.harness_setup_login()}
                        </Button>
                      </div>
                    );
                  })}
                </ModelList>
              </Popover.Popup>
            </Popover.Positioner>
          </Popover.Portal>
        </Popover.Root>
        <Popover.Root
          open={Boolean(callout) && menu === null}
          onOpenChange={(open, { reason }) => {
            if (!open && reason === "escape-key") dismissCallout();
          }}
        >
          <Popover.Portal container={portalContainer}>
            <Popover.Positioner
              className="z-50"
              anchor={modelTriggerRef}
              side="top"
              sideOffset={10}
              collisionPadding={8}
              collisionAvoidance={{ side: "none" }}
              positionMethod="fixed"
            >
              <Popover.Popup
                initialFocus={false}
                finalFocus={false}
                {...calloutHover}
                className="outline-none motion-safe:transition-opacity motion-safe:duration-150 motion-safe:ease-standard data-ending-style:opacity-0 data-starting-style:opacity-0"
                style={callout && ({ "--nudge-from": callout.colors.from, "--nudge-to": callout.colors.to } as CSSProperties)}
              >
                <button
                  type="button"
                  onClick={() => {
                    if (callout) setPointAt(callout.line.key);
                    dismissCallout(true);
                    openModels();
                  }}
                  className="flex w-max max-w-72 items-center gap-2.5 rounded-lg border border-nudge-edge bg-linear-165 from-nudge-start via-nudge-middle via-50% to-background to-88% py-2 ps-3 pe-3.5 text-start shadow-nudge outline-none focus-visible:outline-2 focus-visible:outline-focus"
                >
                  {callout?.mark}
                  <span className="min-w-0">
                    <span className="block text-sm leading-5 font-semibold">{callout && lineName(callout.line)}</span>
                    <span key={callout?.id} ref={fitBalanced} className="block text-xs leading-4 text-balance text-subtext">{callout?.text}</span>
                  </span>
                </button>
                <Popover.Arrow className="-bottom-[5px] size-2 rotate-45 border-r border-b border-nudge-edge bg-background" />
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
            <Tooltip interactive content={m.model_picker_effort_options()} className="rounded-md">
              <Menu.Trigger
                disabled={disabled}
                className={PILL}
                aria-label={[
                  effortLabel ? `${m.model_picker_effort()}: ${effortLabel}` : m.model_picker_effort(),
                  speed ? speedLabel(speed) : null,
                ].filter(Boolean).join(" · ")}
              >
                {speed && <SpeedIcon speed={speed} />}
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
            </Tooltip>
            <Menu.Portal container={portalContainer}>
              <Menu.Positioner className="z-50" side="top" align="end" sideOffset={8} collisionPadding={8} positionMethod="fixed">
                <Menu.Popup aria-label={m.model_picker_effort_options()} className={MENU_POPUP} style={{ width: "min(240px, calc(100vw - 16px))", maxHeight: "min(400px, var(--available-height))" }}>
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
                  {(ultracodeChoice || speeds.length > 0 || variantSwitches) && (effortChoices.length > 0 || variantEfforts.length > 0 || nativeOptions.length > 0) && (
                    <Menu.Separator className={MENU_SEPARATOR} />
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
                  {switchTier && (
                    <Menu.CheckboxItem className={SETTINGS_ROW} checked={speed === switchTier} onCheckedChange={(checked) => pickSpeed(checked ? switchTier : null)}>
                      <span className="inline-flex items-center gap-2">
                        <SpeedIcon speed={switchTier} />
                        {speedLabel(switchTier)}
                      </span>
                      <SwitchIndicator checked={speed === switchTier} aria-hidden="true" />
                    </Menu.CheckboxItem>
                  )}
                  {speeds.length > 1 && !switchTier && (
                    <Menu.Group className="flex min-h-8 items-center justify-between gap-3 px-2 py-1">
                      <Menu.GroupLabel className="inline-flex min-w-0 items-baseline gap-2 text-sm text-text">
                        {m.model_picker_speed()}
                        <span className="truncate text-muted">{speedName(speed)}</span>
                      </Menu.GroupLabel>
                      <Menu.RadioGroup
                        value={speed ?? ""}
                        onValueChange={(next) => { if (typeof next === "string") pickSpeed(next || null); }}
                        className="inline-flex shrink-0 items-center gap-0.5 rounded-md bg-hover-subtle p-0.5"
                      >
                        {speeds.map((tier) => (
                          <Tooltip key={tier ?? ""} interactive content={speedName(tier)} className="flex rounded-sm">
                            <Menu.RadioItem value={tier ?? ""} closeOnClick={false} label={speedName(tier)} aria-label={speedName(tier)} className={SPEED_SEGMENT}>
                              <SpeedIcon speed={tier} />
                            </Menu.RadioItem>
                          </Tooltip>
                        ))}
                      </Menu.RadioGroup>
                    </Menu.Group>
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
        {permissionChoices.length > 0 && (
          <Menu.Root
            open={!disabled && menu === "permissions"}
            modal={false}
            onOpenChange={(open) => setMenu((current) => (open ? "permissions" : current === "permissions" ? null : current))}
          >
            <Tooltip interactive content={permissionSummary} className="rounded-md">
              <Menu.Trigger disabled={disabled} className={PILL} aria-label={permissionSummary}>
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
  placeholder,
  ariaLabelledBy,
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
  /** Shown when no choice is selected instead of falling back to the first choice. */
  placeholder?: string;
  /** Id of an external label element paired with the trigger's value for screen readers. */
  ariaLabelledBy?: string;
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
  const valueId = useId();
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

  const effectiveId = value ?? defaultId ?? (placeholder !== undefined ? null : choices[0]?.id ?? null);
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
  const label = current?.label ?? placeholder ?? choices[0]?.label ?? "";

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
        aria-labelledby={ariaLabelledBy ? `${ariaLabelledBy} ${valueId}` : undefined}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={disabled}
        onClick={activateTrigger}
      >
        <span className="inline-flex min-w-0 items-center gap-2">
          {current && renderIcon?.(current)}
          <span id={ariaLabelledBy ? valueId : undefined} className={cn("truncate", !current && placeholder && "text-muted")}>
            {current ? renderLabel?.(current) ?? label : label}
          </span>
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

/** What the model button points at: a model, the line under its name, and its look. */
interface Callout {
  kind: "release" | "free";
  /** What dismissing it records. */
  id: string;
  line: PickerLine;
  text: string;
  /** CSS colors its gradient runs between. */
  colors: { from: string; to: string };
  mark: ReactNode;
}

/** The labs whose releases the model button points at. */
const FRONTIER_LABS = ["claude", "openai", "gemini"] as const;

/**
 * Each lab's colors: Claude's orange, Gemini's blue to violet. OpenAI's tiers are named for the sky, so each takes
 * its own: Sol the sun, Luna the moon, Terra the earth, Astra the stars; any other is neutral.
 */
function releaseColors(line: PickerLine) {
  const lab = lineLab(line);
  const words = line.family.split(" ");
  const pair = (from: string, to: string) => ({ from: `var(--accent-${from})`, to: `var(--accent-${to})` });
  if (lab === "claude") return pair("orange", "amber");
  if (lab === "gemini") return pair("blue", "purple");
  if (words.includes("sol")) return pair("amber", "orange");
  if (words.includes("luna")) return pair("indigo", "blue");
  if (words.includes("terra")) return pair("green", "teal");
  if (words.includes("astra")) return pair("purple", "indigo");
  return { from: "var(--muted)", to: "var(--border)" };
}

/** Narrows balanced text to its widest line, which `text-wrap: balance` leaves at the width it had unbalanced. */
function fitBalanced(text: HTMLElement | null) {
  if (!text) return;
  text.style.width = "";
  const range = document.createRange();
  range.selectNodeContents(text);
  const lines = [...range.getClientRects()];
  if (lines.length > 1) text.style.width = `${Math.ceil(Math.max(...lines.map((line) => line.width)))}px`;
}

/** The workspaces whose model button dismissed a callout since the app loaded. */
const calledOut = new WeakSet<object>();
/** The callouts reported shown since the app loaded. */
const shownCallouts = new Set<string>();

const NO_RELEASES: Record<string, string> = {};

const shortDay = (day: Date) => new Intl.DateTimeFormat(getLocale(), { month: "short", day: "numeric" }).format(day);

/** What the nudge says under its model's name; nothing once its free days are over, or for a kind this version doesn't know. */
function nudgeMessage({ kind, until, message }: ModelNudge) {
  if (kind !== "new" && kind !== "trial") return undefined;
  const last = until ? localDay(until) : undefined;
  if (last && Date.now() >= new Date(last.getFullYear(), last.getMonth(), last.getDate() + 1).getTime()) return undefined;
  if (message) return message;
  if (kind === "new") return m.model_picker_nudge_new();
  if (!last) return m.model_picker_nudge_trial();
  return m.model_picker_nudge_trial_until({ date: shortDay(last) });
}

/** What the release callout says under its model's name: who made it and when, within the week it's new. */
function releaseText(line: PickerLine, day: string) {
  const lab = LABS.find((entry) => entry.lab === lineLab(line))!.name;
  const today = new Date();
  const days = Math.round((new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime() - localDay(day).getTime()) / 86_400_000);
  if (days === 0) return m.model_picker_released_today({ lab });
  if (days === 1) return m.model_picker_released_yesterday({ lab });
  return m.model_picker_released_this_week({ lab });
}

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
