import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { Check, ChevronDown, ChevronLeft, ChevronRight, Lock, Zap } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import {
  getHarnesses,
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
} from "../api";
import { renderNote } from "./agentNote";
import { HarnessLogo } from "./HarnessLogo";
import { onHarnessAuth } from "../events";
import type { AgentSelection } from "../api";
import { COMPOSER_CONTROL_CLASS_NAME, MODEL_ITEM_CLASS_NAME } from "../styleClasses";

const MODEL_GROUP_CLASS_NAME = [
  "model-group flex items-center justify-between gap-2",
  "text-md font-semibold text-text pt-2.5 px-2 pb-1.5",
].join(" ");

const MODEL_MORE_CLASS_NAME = [
  "model-more [&_code]:font-mono [&_code]:text-xs",
  "[&_code]:bg-panel [&_code]:border [&_code]:border-border-variant",
  "[&_code]:rounded-xs [&_code]:py-px [&_code]:px-[5px] [&_code]:whitespace-nowrap",
  "pt-1 px-2 pb-2 text-xs text-muted",
].join(" ");

export type ModelSelection = AgentSelection;

export const HARNESS_LABELS: Record<HarnessId, string> = {
  "claude-code": "Claude Code",
  codex: "Codex",
  opencode: "OpenCode",
};

/** First harness that can actually run — the fallback when nothing is picked.
 * Seeds mode/reasoning from that harness's advertised defaults. */
export function defaultSelection(harnesses: Harness[]): ModelSelection | null {
  const ready = harnesses.find((h) => h.agentReady);
  if (!ready) return null;
  // Seed the catalog's first model — the catalogs are CLI-discovered now, so
  // it's a real, current id. Custom providers advertise no models; they seed
  // null (= send no `--model`, the CLI uses whatever it is configured with).
  const model = ready.models[0]?.id ?? null;
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
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      // Escape closes the picker and must NOT bubble to other document-level
      // Escape handlers (e.g. ChatPanel's stop-streaming listener) — closing an
      // open picker shouldn't also interrupt an in-flight turn. Capture phase +
      // stopPropagation makes this order-independent: relying on registration
      // order and defaultPrevented among bubble-phase document listeners is
      // fragile, since whichever registered first runs first.
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        setOpen(false);
        triggerRef?.current?.focus();
      }
    };
    document.addEventListener("mousedown", onDown, true);
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("mousedown", onDown, true);
      document.removeEventListener("keydown", onKey, true);
    };
  }, [open, triggerRef]);
  return { open, setOpen, ref };
}

/** Composer selector: pick the harness + model new sessions (and same-harness
 * turns) run on. Groups mirror the Harnesses settings tab. */
export function ModelPicker({
  value,
  onSelect,
  permissionChoices = [],
  defaultPermissionId,
  onSelectPermission,
  reasoningChoices = [],
  defaultReasoningId,
  onSelectReasoning,
  onHarnesses,
  lockHarness = false,
}: {
  value: ModelSelection | null;
  onSelect: (value: ModelSelection) => void;
  permissionChoices?: OptionChoice[];
  defaultPermissionId?: string | null;
  onSelectPermission?: (id: string) => void;
  reasoningChoices?: OptionChoice[];
  defaultReasoningId?: string | null;
  onSelectReasoning?: (id: string) => void;
  onHarnesses?: (harnesses: Harness[]) => void;
  /** When set (a session is open), only the current harness is offered — its
   * harness is fixed for its lifetime, so you can still switch models within it
   * but not switch to a different harness. */
  lockHarness?: boolean;
}) {
  const [harnesses, setHarnesses] = useState<Harness[]>([]);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const submenuHeaderRef = useRef<HTMLButtonElement>(null);
  const { open, setOpen, ref: rootRef } = usePopover(triggerRef);
  const [filter, setFilter] = useState("");
  const [page, setPage] = useState<"root" | "models" | "reasoning" | "speed" | "permissions">("root");

  const close = () => {
    setOpen(false);
    setPage("root");
    setFilter("");
  };

  useEffect(() => {
    if (open && (page === "reasoning" || page === "speed" || page === "permissions")) {
      submenuHeaderRef.current?.focus();
    }
  }, [open, page]);

  useEffect(() => {
    let mounted = true;
    const load = (refresh = false) =>
      getHarnesses(refresh)
        .then((list) => {
          if (!mounted) return;
          setHarnesses(list);
          onHarnesses?.(list);
        })
        .catch(() => {});
    void load();
    const unsubscribe = onHarnessAuth(() => void load(true));
    return () => {
      mounted = false;
      unsubscribe();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const groups = useMemo(() => {
    const q = filter.trim().toLowerCase();
    // Locked to the open session's harness: only offer that one.
    const shown =
      lockHarness && value ? harnesses.filter((h) => h.id === value.harness) : harnesses;
    return shown.map((h) => {
      let models = h.models;
      if (q) models = models.filter((m) => m.id.toLowerCase().includes(q));
      // opencode's long tail (openrouter etc.) stays behind the filter box.
      else if (h.id === "opencode") models = models.slice(0, 6);
      return { harness: h, models, hidden: q ? 0 : h.models.length - models.length };
    });
  }, [harnesses, filter, lockHarness, value]);

  /** Switch harness → reseed mode defaults for that harness.
   *
   * Reasoning is reconciled against the *newly selected model* in both cases:
   * choices are per-model now, so an effort the previous model allowed may not
   * exist on this one (`ultra` on Sol → 5.5). Keeping it would send a value the
   * model rejects, so it falls back to that model's default. */
  const pick = (harness: Harness, model: string | null) => {
    const sameHarness = value?.harness === harness.id;
    onSelect({
      harness: harness.id,
      model,
      serviceTier: reconcileServiceTier(
        harness,
        model,
        sameHarness ? value?.serviceTier : null,
      ),
      permissionMode: sameHarness
        ? value!.permissionMode
        : harness.options?.defaultPermissionMode ?? null,
      reasoningLevel: reconcileReasoning(
        harness,
        model,
        sameHarness ? value!.reasoningLevel : null,
      ),
    });
    close();
  };

  // Pill label: prefer the catalog's own name for the selected model (the
  // Claude aliases like `opus[1m]` are meaningless prettified).
  const selected =
    value?.model != null
      ? harnesses
          .find((h) => h.id === value.harness)
          ?.models.find((m) => m.id === value.model)
      : undefined;
  const label = value
    ? value.model
      ? selected
        ? harnessModelLabel(selected)
        : modelLabel(value.model)
      : m.model_picker_default_model()
    : m.model_picker_model();
  const effectiveReasoningId = value?.reasoningLevel ?? defaultReasoningId ?? reasoningChoices[0]?.id;
  const reasoningLabel = reasoningChoices.find((choice) => choice.id === effectiveReasoningId)?.label;
  const effectivePermissionId = value?.permissionMode ?? defaultPermissionId ?? permissionChoices[0]?.id;
  const permissionLabel = permissionChoices.find((choice) => choice.id === effectivePermissionId)?.label;
  const reasoningAxisLabel = value?.harness === "opencode" ? m.model_picker_variant() : m.model_picker_effort();
  const selectedHarness = harnesses.find((harness) => harness.id === value?.harness);
  const speedChoices = serviceTiersFor(selectedHarness, value?.model);
  const effectiveServiceTier = reconcileServiceTier(
    selectedHarness,
    value?.model,
    value?.serviceTier,
  );
  const speedLabel = speedChoices.find((choice) => choice.id === effectiveServiceTier)?.label;

  const chooseReasoning = (id: string) => {
    onSelectReasoning?.(id);
    close();
  };

  const choosePermission = (id: string) => {
    onSelectPermission?.(id);
    close();
  };

  const chooseSpeed = (id: string) => {
    if (value) onSelect({ ...value, serviceTier: id });
    close();
  };

  const menuRow = (
    title: string,
    detail: string | undefined,
    next: typeof page,
  ) => (
    <button
      type="button"
      className="model-root-row flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-start text-md text-text hover:bg-surface"
      aria-haspopup="menu"
      onClick={() => setPage(next)}
    >
      <span className="flex-1">{title}</span>
      {detail && <span className="max-w-36 truncate text-sm text-muted">{detail}</span>}
      <ChevronRight size={14} className="shrink-0 text-muted" />
    </button>
  );

  const submenuHeader = (title: string) => (
    <button
      ref={submenuHeaderRef}
      type="button"
      className="model-submenu-header flex w-full items-center gap-2 border-0 border-b border-solid border-b-border-variant bg-transparent px-2 py-2 text-start text-sm font-medium text-text hover:bg-surface"
      onClick={() => {
        setPage("root");
        setFilter("");
      }}
    >
      <ChevronLeft size={15} />
      {title}
    </button>
  );

  const choiceList = (
    choices: OptionChoice[],
    effectiveId: string | undefined,
    defaultId: string | null | undefined,
    choose: (id: string) => void,
  ) => (
    <div className="model-menu-list overflow-y-auto p-1.5">
      {choices.map((choice) => (
        <button key={choice.id} className={MODEL_ITEM_CLASS_NAME} onClick={() => choose(choice.id)}>
          <span className="flex min-w-0 flex-col items-start gap-0.5">
            <span>
              {choice.label}
              {choice.id === defaultId && (
                <span className="font-normal text-muted"> {m.model_picker_default()}</span>
              )}
            </span>
            {choice.description && (
              <span className="max-w-72 text-sm font-normal leading-snug text-muted">
                {choice.description}
              </span>
            )}
          </span>
          {choice.id === effectiveId && <Check size={13} />}
        </button>
      ))}
    </div>
  );

  return (
    <div className="model-picker relative inline-flex min-w-0" data-onboarding="model-picker" ref={rootRef}>
      <button
        ref={triggerRef}
        type="button"
        className={`${COMPOSER_CONTROL_CLASS_NAME} composer-pill min-w-0 max-w-full gap-[5px] px-2 text-md text-text whitespace-nowrap`}
        title={m.a11y_chat_model({ label: `${label}${reasoningLabel ? ` · ${reasoningLabel}` : ""}${speedLabel ? ` · ${speedLabel}` : ""}` })}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => {
          if (open) close();
          else {
            setPage("root");
            setOpen(true);
          }
        }}
      >
        {effectiveServiceTier === "priority" ? (
          <Zap size={14} fill="currentColor" aria-hidden="true" />
        ) : value?.harness ? (
          <HarnessLogo harness={value.harness} size={14} />
        ) : null}
        {effectiveServiceTier === "priority" && <span className="sr-only">{m.model_picker_fast_speed()} </span>}
        <span className="model-picker-label min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">
          {label}
          {reasoningLabel && <span className="model-picker-reasoning ms-1 text-muted">{reasoningLabel}</span>}
        </span>
        <ChevronDown size={14} className="shrink-0 text-muted" />
      </button>
      {open && (
        <div className="model-menu absolute bottom-[calc(100%_+_8px)] start-0 max-h-100 flex flex-col bg-background border border-border rounded-md shadow-[0_10px_26px_rgba(0,_0,_0,_0.16)] z-50 overflow-hidden w-72 [&.align-right]:start-auto [&.align-right]:end-0 [&_input]:rounded-none [&_input]:border-0 [&_input]:border-b [&_input]:border-b-border-variant [&_input]:bg-none [&_input]:bg-transparent [&_input]:py-2 [&_input]:px-2.5 [&_input]:text-sm [&_input]:outline-none align-right">
          {page === "root" && (
            <div className="model-root-menu p-1">
              {menuRow(m.model_picker_model(), label, "models")}
              {reasoningChoices.length > 0 && menuRow(reasoningAxisLabel, reasoningLabel, "reasoning")}
              {speedChoices.length > 0 && menuRow(m.model_picker_speed(), speedLabel, "speed")}
              {permissionChoices.length > 0 && menuRow(m.model_picker_mode(), permissionLabel, "permissions")}
            </div>
          )}
          {page === "models" && (
            <>
              {submenuHeader(m.model_picker_model())}
              <input
                autoFocus
                type="text"
                placeholder={m.model_picker_search_models()}
                value={filter}
                onChange={(e) => setFilter(e.target.value)}
              />
              <div className="model-menu-list overflow-y-auto p-1.5">
                {groups.map(({ harness, models, hidden }) => (
                  <div key={harness.id} className="[&_.model-item]:ps-6">
                    <div className={MODEL_GROUP_CLASS_NAME}>
                      <span className="inline-flex items-center gap-1.5">
                        <HarnessLogo harness={harness.id} size={14} />
                        {harness.name}
                      </span>
                      {!harness.agentReady && (
                        <span className="model-group-status inline-flex items-center gap-1 text-accent-amber font-normal">
                          <Lock size={10} /> {m.model_picker_unavailable()}
                        </span>
                      )}
                    </div>
                    {!harness.agentReady ? (
                      <div className="model-more [&_code]:font-mono [&_code]:text-xs [&_code]:bg-panel [&_code]:border [&_code]:border-border-variant [&_code]:rounded-xs [&_code]:py-px [&_code]:px-[5px] [&_code]:whitespace-nowrap pt-1 px-2 pb-2 text-xs text-muted model-unavailable leading-normal border-b border-b-border-variant">
                        {harness.agentNote ? renderNote(harness.agentNote) : m.model_picker_not_available()}
                      </div>
                    ) : (
                      <>
                    {/* "Default model" (= send no --model, the CLI decides)
                        only where the CLI advertises no catalog — a custom
                        provider whose real models live behind its gateway.
                        With a discovered catalog the row is redundant noise:
                        the catalog's own default leads the list. */}
                    {harness.models.length === 0 && (
                      <button className={MODEL_ITEM_CLASS_NAME} onClick={() => pick(harness, null)}>
                        <span>
                          {m.model_picker_default_model()}
                          <span className="model-id">{m.model_picker_cli_configuration()}</span>
                        </span>
                        {value?.harness === harness.id && value?.model === null && (
                          <Check size={13} />
                        )}
                      </button>
                    )}
                    {models.map((m) => (
                      <button
                        key={m.id}
                        className={MODEL_ITEM_CLASS_NAME}
                        title={m.id}
                        onClick={() => pick(harness, m.id)}
                      >
                        <span>{harnessModelLabel(m)}</span>
                        {value?.harness === harness.id && value?.model === m.id && (
                          <Check size={13} />
                        )}
                      </button>
                    ))}
                    {hidden > 0 && (
                      <div className={MODEL_MORE_CLASS_NAME}>{m.model_picker_more({ count: fmtNumber(hidden) })}</div>
                    )}
                    {/* Free-form escape hatch: the catalogs are curated menus,
                        not the set of ids the CLIs accept — `--model
                        claude-opus-5` works on a CLI whose menu doesn't list
                        it. Typing an id not in the list offers it directly. */}
                    {filter.trim().length > 0 &&
                      !harness.models.some((m) => m.id === filter.trim()) && (
                        <button
                          className={MODEL_ITEM_CLASS_NAME}
                          onClick={() => pick(harness, filter.trim())}
                        >
                          <span>
                            {m.model_picker_use_id({ id: ltr(filter.trim()) })}
                          </span>
                        </button>
                      )}
                      </>
                    )}
                  </div>
                ))}
                {harnesses.length === 0 && <div className={MODEL_MORE_CLASS_NAME}>{m.model_picker_detecting_harnesses()}</div>}
              </div>
              {lockHarness && value && harnesses.length > 1 && (
                <div className="model-locked-note flex items-center gap-1.5 py-[7px] px-3 text-xs text-muted border-t border-t-border-variant [&_svg]:shrink-0">
                  <Lock size={11} />
                  {m.model_picker_sessions_keep_their_harness_new_chat_to_switch()}
                </div>
              )}
            </>
          )}
          {page === "reasoning" && (
            <>
              {submenuHeader(reasoningAxisLabel)}
              {choiceList(reasoningChoices, effectiveReasoningId, defaultReasoningId, chooseReasoning)}
            </>
          )}
          {page === "permissions" && (
            <>
              {submenuHeader(m.model_picker_mode())}
              {choiceList(permissionChoices, effectivePermissionId, defaultPermissionId, choosePermission)}
            </>
          )}
          {page === "speed" && (
            <>
              {submenuHeader(m.model_picker_speed())}
              {choiceList(speedChoices, effectiveServiceTier ?? undefined, "default", chooseSpeed)}
            </>
          )}
        </div>
      )}
    </div>
  );
}

/** A compact single-axis dropdown (permission mode or reasoning level). Renders
 * nothing when the harness advertises no choices for this axis. Mirrors the
 * Claude Code composer menu: a header, the current-default row pinned at top
 * with a "· Default" note, then the full numbered list. */
export function OptionPicker({
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
  renderIcon,
  onSelect,
}: {
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
  renderIcon?: (choice: OptionChoice) => ReactNode;
  onSelect: (id: string) => void;
}) {
  const { open, setOpen, ref } = usePopover();
  if (choices.length === 0) return null;

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
  const label = current?.label ?? choices[0]?.label ?? "";

  const choose = (id: string) => {
    onSelect(id);
    setOpen(false);
  };

  return (
    <div className={`option-picker relative inline-flex${variant === "field" ? " w-full" : ""}`} ref={ref}>
      <button
        type="button"
        className={
          variant === "field"
            ? "inline-flex h-9 w-full items-center justify-between gap-2 rounded-md border border-border bg-background px-3 text-sm font-normal text-text transition-colors duration-120 ease-standard hover:bg-surface disabled:opacity-45"
            : `${COMPOSER_CONTROL_CLASS_NAME} ${variant === "pill" ? "composer-pill gap-[5px] px-2 text-md text-text whitespace-nowrap" : "composer-bare gap-[3px] px-1 text-md text-text"}`
        }
        title={title}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((v) => !v)}
      >
        <span className="inline-flex min-w-0 items-center gap-2">
          {current && renderIcon?.(current)}
          <span className="truncate">{label}</span>
        </span>
        <ChevronDown size={12} />
      </button>
      {open && (
        <div className={`option-menu absolute bottom-[calc(100%_+_8px)] start-0 max-h-95 flex flex-col bg-background border border-border rounded-lg shadow-[0_12px_32px_rgba(0,_0,_0,_0.18)] z-50 overflow-hidden min-w-47.5 p-1.5 [&.align-right]:start-auto [&.align-right]:end-0 [&.drop-down]:bottom-auto [&.drop-down]:top-[calc(100%_+_4px)] [&.session-menu]:start-auto [&.session-menu]:end-1.5 [&.session-menu]:top-[calc(100%_-_2px)] [&.session-menu]:min-w-35 ${choices.some((choice) => choice.description) ? "min-w-80" : ""} ${variant === "field" ? "min-w-full" : ""} ${align === "right" ? "align-right" : ""} ${dropDown ? "drop-down" : ""}`}>
          {header && <div className={MODEL_GROUP_CLASS_NAME}>{header}</div>}
          {pinned && (
            <>
              <button type="button" className={MODEL_ITEM_CLASS_NAME} onClick={() => choose(pinned.id)}>
                <span className="inline-flex items-center gap-2">
                  {renderIcon?.(pinned)}
                  <span>
                    {pinned.label}
                    {/* An unnamed sentinel's label already IS "Default", so the
                        usual marker would read "Default · Default" — say where
                        the behavior comes from instead. A named one ("Adaptive")
                        gets the standard marker. */}
                    <span className="option-default text-muted font-normal">
                      {m.model_picker_cli_configuration()}
                    </span>
                  </span>
                </span>
                {effectiveId === pinned.id && <Check size={13} />}
              </button>
              <div className="option-sep h-px my-[5px] mx-1 bg-border-variant" />
            </>
          )}
          {rest.map((c, i) => (
            <button type="button" key={c.id} className={MODEL_ITEM_CLASS_NAME} onClick={() => choose(c.id)}>
              <span className="flex min-w-0 items-center gap-2">
                {renderIcon?.(c)}
                <span className="flex min-w-0 flex-col items-start gap-0.5">
                  <span>
                    {c.label}
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
                <Check size={13} />
              ) : (
                numbered && <span className="option-num text-muted text-xs tabular-nums">{i + 1}</span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
