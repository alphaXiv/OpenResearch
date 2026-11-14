import { useContext, useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Popover } from "@base-ui/react/popover";
import { DirectionProvider } from "@base-ui/react/direction-provider";
import { fmtTokens, type ContextUsage, type HarnessId } from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import { useWorkspace } from "../workspace";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { Button } from "./ui";
import { cn } from "./ui/cn";
import { fmtEstimate, fmtMoney, fmtPercent, leftTone, lowestWindow, LOW_PERCENT, PLAN_GRID, PlanRows, Ring, UsageGauge, usePlanAgent, useUpdateAgent } from "./Usage";

/** Amber at 80% of the context window, red at 95%. */
function contextTone(percent: number) {
  if (percent >= 95) return { stroke: "var(--accent-red)", fill: "bg-accent-red" };
  if (percent >= 80) return { stroke: "var(--accent-amber)", fill: "bg-accent-amber" };
  return { stroke: "var(--subtext)", fill: "bg-subtext" };
}

const RUNNING_REFETCH_MS = 10_000;

/**
 * The composer's usage: a ring for the chat's context, then at most one text (its plan running low, or what a
 * pay-per-token chat has cost); its menu holds the context, the plan of the chat's account and what this chat cost.
 */
export function UsageMenu({ usage, busy, harness, profile, sessionId, remote, onCompact }: {
  usage?: ContextUsage;
  /** A turn is running; the chat's cost is read again when it ends. */
  busy: boolean;
  harness: HarnessId | undefined;
  profile: string | null | undefined;
  /** The open chat; null before the first message. */
  sessionId: string | null;
  remote: boolean;
  /** Present where the agent compacts on request. */
  onCompact?: () => void;
}) {
  const { portalContainer } = useContext(WorkspaceDomContext);
  const [open, setOpen] = useState(false);
  const usageQuery = useQuery({
    ...useWorkspace().queries.getUsageQuery(sessionId),
    refetchInterval: (query) => open && query.state.data?.chat?.compute?.running ? RUNNING_REFETCH_MS : false,
  });
  const report = usageQuery.data;
  const { refetch } = usageQuery;
  const wasBusy = useRef(busy);
  useEffect(() => {
    if (wasBusy.current && !busy) void refetch();
    wasBusy.current = busy;
  }, [busy, refetch]);
  const planAgent = usePlanAgent();
  const updateAgent = useUpdateAgent(remote);

  const plan = harness ? report?.plans?.find((one) => one.harness === harness && (one.profile ?? null) === (profile ?? null)) : undefined;
  const used = usage?.usedTokens ?? 0;
  const contextWindow = used > 0 && usage?.contextWindow ? usage.contextWindow : null;
  const contextPercent = contextWindow ? Math.min(100, (used / contextWindow) * 100) : null;
  // On a plan the cost is only what the tokens would have cost at API prices, which the menu leaves
  // out; while the plans are unread, whether this chat is on one is unknown.
  const billed = report?.plans && !plan ? report.chat?.agent.filter((spent) => spent.micros > 0) ?? [] : [];
  const cost = billed.length > 0 ? billed.map((spent) => fmtMoney(spent.micros, spent.currency)).join(" + ") : null;
  const compute = report?.chat?.compute?.runs ? report.chat.compute : null;
  if (used <= 0 && !plan && !cost && !compute) return null;

  const low = plan && lowestWindow(plan);
  const after = low && low.remainingPercent <= LOW_PERCENT
    ? { text: m.usage_percent_left({ percent: fmtPercent(low.remainingPercent) }), tone: leftTone(low.remainingPercent).text }
    : cost ? { text: cost, tone: "" } : null;
  const compactAt = contextWindow && usage?.compactAt && usage.compactAt < contextWindow ? usage.compactAt : null;
  const plural = (count: number) => new Intl.PluralRules(getLocale()).select(count) === "one";
  const runs = compute && (plural(compute.runs) ? m.usage_chat_compute_one : m.usage_chat_compute_other)({ count: compute.runs });
  const estimate = compute && compute.costMicros > 0 ? fmtEstimate(compute.costMicros, compute.currency) : null;
  const computeAmount = estimate && compute?.running ? m.usage_so_far({ amount: estimate }) : estimate;
  const computeCost = !compute ? null
    : compute.unpriced >= compute.runs ? m.usage_cost_unavailable()
      : [computeAmount, compute.unpriced > 0 && (plural(compute.unpriced) ? m.usage_runs_unpriced_one : m.usage_runs_unpriced_other)({ count: compute.unpriced })].filter(Boolean).join(" · ");

  const sections = [
    used > 0 && <section key="context">
      <div className="flex items-baseline justify-between gap-3">
        <span className="font-medium text-text">{m.usage_context()}</span>
        <span className="tabular-nums text-text">{contextWindow
          ? m.usage_context_of({ used: ltr(fmtTokens(used)), total: ltr(fmtTokens(contextWindow)) })
          : m.usage_context_tokens({ value: ltr(fmtTokens(used)) })}</span>
      </div>
      {contextWindow && contextPercent !== null && <div
        role="meter"
        aria-label={m.usage_context()}
        aria-valuenow={Math.round(contextPercent)}
        aria-valuemin={0}
        aria-valuemax={100}
        className="relative mt-2 h-[5px] rounded-full bg-border"
      >
        <span className={cn("absolute inset-y-0 start-0 rounded-full", contextTone(contextPercent).fill)} style={{ width: `${Math.max(contextPercent, 1)}%` }} />
        {compactAt && <>
          <span aria-hidden="true" className="absolute inset-y-0 end-0 rounded-e-full opacity-60" style={{ insetInlineStart: `${(compactAt / contextWindow) * 100}%`, backgroundImage: "repeating-linear-gradient(135deg, var(--subtext) 0 1px, transparent 1px 3px)" }} />
          <span aria-hidden="true" data-compact-at className="absolute -inset-y-[3px] -ms-px w-0.5 rounded-full bg-text" style={{ insetInlineStart: `${(compactAt / contextWindow) * 100}%` }} />
        </>}
      </div>}
      {(compactAt || onCompact) && <div className="mt-2 flex min-h-7 items-center justify-between gap-3">
        <span className="text-xs text-subtext">{compactAt && compactAt > used ? m.usage_left_before_compacting({ tokens: ltr(fmtTokens(compactAt - used)) }) : null}</span>
        {onCompact && <Button size="small" onClick={() => { setOpen(false); onCompact(); }}>{m.usage_compact()}</Button>}
      </div>}
    </section>,
    plan && <section key="plan" className={PLAN_GRID}>
      <PlanRows plan={plan} agent={planAgent(plan)} first onUpdate={updateAgent(plan)} />
    </section>,
    (cost || compute) && <section key="chat" className="grid gap-1">
      {cost && <div className="flex items-baseline justify-between gap-3">
        <span className="text-subtext">{m.usage_chat()}</span>
        <span className="tabular-nums text-text">{cost}</span>
      </div>}
      {compute && <div className="flex items-baseline justify-between gap-3">
        <span className="flex items-center gap-1.5 text-subtext">{compute.running && <span data-running className="size-1.5 rounded-full bg-accent-green" aria-hidden="true" />}{runs}</span>
        <span className={cn("tabular-nums", computeAmount ? "text-text" : "text-subtext")}>{computeCost}</span>
      </div>}
    </section>,
  ].filter(Boolean);

  return <DirectionProvider direction={typeof document !== "undefined" && document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
    <Popover.Root open={open} onOpenChange={(next) => {
      setOpen(next);
      if (next) void usageQuery.refetch();
    }}>
      <Popover.Trigger
        type="button"
        data-usage-trigger
        aria-label={after ? `${m.usage_menu()} · ${after.text}` : m.usage_menu()}
        className="composer-bare inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md px-1.5 text-sm tabular-nums text-subtext transition-[background,color] duration-150 ease-standard hover:bg-hover-muted hover:text-text data-popup-open:bg-hover-muted data-popup-open:text-text"
      >
        {contextPercent !== null
          ? <Ring size={16} r={6.5} width={2.5} percent={contextPercent} stroke={contextTone(contextPercent).stroke} />
          : used > 0 ? <span>{fmtTokens(used)}</span> : <UsageGauge />}
        {after && <span className={after.tone}>{after.text}</span>}
      </Popover.Trigger>
      <Popover.Portal container={portalContainer}>
        <Popover.Positioner side="top" align="end" sideOffset={8} collisionPadding={8} className="z-50">
          <Popover.Popup aria-label={m.usage_menu()} className="outline-none w-78 max-w-[calc(100vw-1rem)] max-h-(--available-height) overflow-y-auto rounded-lg border border-border bg-background px-3 text-sm shadow-menu">
            {sections.map((section, index) => <div key={index} className={cn("py-2.5", index > 0 && "border-t border-border-variant")}>{section}</div>)}
          </Popover.Popup>
        </Popover.Positioner>
      </Popover.Portal>
    </Popover.Root>
  </DirectionProvider>;
}
