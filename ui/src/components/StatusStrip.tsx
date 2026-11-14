import { useContext, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { useMutate } from "@clo/react-mutation";
import { useQuery } from "@tanstack/react-query";
import { Popover } from "@base-ui/react/popover";
import { RotateCw } from "lucide-react";
import type { Harness, PlanUsage, RuntimeInfo, UsageReport } from "../api";
import { timeAgo } from "../api";
import { autoDir, fmtClock } from "../i18n";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import { mutRefreshUsage } from "../queries/settings";
import { useWorkspace } from "../workspace";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { AccountChip } from "./AccountChip";
import { BackendLogo, backendName } from "./BackendLogos";
import { Button, IconButton, LoadingRow, SEGMENTED_CLASS_NAME, Spinner, Tooltip } from "./ui";
import { cn } from "./ui/cn";
import { fmtEstimate, fmtMoney, leftTone, lowestWindow, LOW_PERCENT, PLAN_GRID, PlanMark, PlanRows, planSummary, Ring, UsageGauge, usePlanAgent, useUpdateAgent } from "./Usage";
import { useMachineState, WorkspaceConnection } from "./WorkspaceConnection";

/** The sidebar's footer: you, your agents' plans, then the machine, which grows into what's left. */
export function StatusStrip({ runtime, onDetails }: { runtime: RuntimeInfo; onDetails: () => void }) {
  const { current, live } = useMachineState(runtime);
  const usage = useQuery(useWorkspace().queries.getUsageQuery());
  const report = usage.data;
  const hasUsage = report ? report.plans === null || report.plans.length > 0 || spendSeries(report, String).length > 0 || spendSeries(report).length > 0 : false;
  // Failing to read usage is not having none, so the segment stays to say so; an older `orx up` has no route.
  const shown = usage.error != null || hasUsage;
  const machineFilled = current !== null && live && current.state !== "disconnected";

  const stripRef = useRef<HTMLDivElement>(null);
  const [showName, setShowName] = useState(true);

  // The name shows only whole, and gives way before the machine's name does.
  useLayoutEffect(() => {
    const strip = stripRef.current;
    const machine = strip?.querySelector<HTMLElement>("[data-machine-name]");
    if (!strip || !machine) return;

    const fit = () => {
      const parts = [...machine.children].map((child) => child.getBoundingClientRect());
      const used = parts.length ? Math.max(...parts.map((part) => part.right)) - Math.min(...parts.map((part) => part.left)) : 0;
      const slack = machine.getBoundingClientRect().width - used;
      const label = machine.firstElementChild;
      const name = strip.querySelector("[data-account-name]");
      const ruler = strip.querySelector<HTMLElement>("[data-account-ruler]");
      const clipped = (element: Element | null) => !!element && element.scrollWidth > element.clientWidth + 0.5;
      if (name) {
        if (clipped(name) || clipped(label)) setShowName(false);
      } else if (ruler && slack >= ruler.offsetWidth + 14) setShowName(true);
    };

    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(strip);
    observer.observe(machine);
    return () => observer.disconnect();
  }, [showName, !!shown]);

  return <div ref={stripRef} className="@container relative flex shrink-0 items-center gap-0.5 overflow-hidden border-t border-border p-1">
    <AccountChip showName={showName} onDetails={onDetails} />
    {shown && <>
      <Separator />
      <UsageSegment
        report={report ?? undefined}
        error={usage.error}
        onOpen={() => void usage.refetch()}
        machine={runtime.kind === "ssh" ? autoDir(runtime.session.host) : m.projects_local()}
        remote={runtime.kind === "ssh"}
        stale={!live || (current !== null && current.state !== "connected")}
        fetchedAt={usage.dataUpdatedAt}
      />
    </>}
    {!machineFilled && <Separator />}
    <WorkspaceConnection runtime={runtime} />
  </div>;
}

function Separator() {
  return <span className="mx-0.5 h-3.5 w-px shrink-0 bg-border @max-[230px]:hidden" aria-hidden="true" />;
}

function UsageSegment({ report, error, onOpen, machine, remote, stale, fetchedAt }: {
  /** The last report read, if any. */
  report: UsageReport | undefined;
  /** Why the latest read failed; the panel then shows `report` as of its time. */
  error: Error | null;
  onOpen: () => void;
  machine: string;
  remote: boolean;
  stale: boolean;
  fetchedAt: number;
}) {
  const { portalContainer } = useContext(WorkspaceDomContext);
  const planAgent = usePlanAgent();
  const failed = error != null;
  const plans = report?.plans ?? [];
  const summaries = plans.map((plan) => planSummary(plan, planAgent(plan)));
  const label = summaries.length
    ? m.usage_trigger_plans({ machine, plans: new Intl.ListFormat(getLocale(), { type: "unit" }).format(summaries) })
    : m.usage_on({ machine });

  return <Popover.Root onOpenChange={(open) => { if (open) onOpen(); }}>
    <Tooltip interactive content={m.usage_title()} className="shrink-0">
      <Popover.Trigger
        aria-label={label}
        className={cn("flex h-7 shrink-0 items-center gap-1.5 rounded-md px-1.5 text-sm text-subtext hover:bg-surface hover:text-text data-popup-open:bg-surface data-popup-open:text-text focus-visible:outline-2 focus-visible:outline-focus", (stale || failed) && "opacity-50")}
      >
        {plans.length > 0 ? <PlanRings plans={plans} planAgent={planAgent} /> : <>
          <UsageGauge />
          <span className="@max-[280px]:hidden">{m.usage_title()}</span>
        </>}
      </Popover.Trigger>
    </Tooltip>
    <Popover.Portal container={portalContainer}>
      <Popover.Positioner side="top" align="start" sideOffset={6} collisionPadding={8} className="z-50">
        <Popover.Popup aria-label={m.usage_on({ machine })} className="outline-none w-80 max-w-[calc(100vw-1rem)] max-h-(--available-height) overflow-y-auto rounded-lg border border-border bg-background px-3 text-sm shadow-menu">
          {report
            ? <UsagePanel report={report} machine={machine} remote={remote} stale={stale || failed} fetchedAt={fetchedAt} failure={failed && <ReadFailed error={error} onRetry={onOpen} />} />
            : <div className="py-2.5">
              <span dir="auto" className="block truncate text-xs font-medium text-subtext">{m.usage_on({ machine })}</span>
              {failed && <ReadFailed error={error} onRetry={onOpen} />}
            </div>}
        </Popover.Popup>
      </Popover.Positioner>
    </Popover.Portal>
  </Popover.Root>;
}

const MAX_RINGS = 3;

/** One ring per plan around its mark; past three, or two in a narrow strip, the rest fold into "+n". */
function PlanRings({ plans, planAgent }: { plans: PlanUsage[]; planAgent: (plan: PlanUsage) => Harness | undefined }) {
  return <span className="flex items-center gap-1">
    {plans.slice(0, MAX_RINGS).map((plan, index) => <span key={`${plan.harness}:${plan.profile ?? ""}`} className={index === MAX_RINGS - 1 ? "@max-[250px]:hidden" : undefined}>
      <PlanRing plan={plan} agent={planAgent(plan)} />
    </span>)}
    <MoreRings folded={plans.slice(MAX_RINGS)} className="@max-[250px]:hidden" />
    <MoreRings folded={plans.slice(MAX_RINGS - 1)} className="hidden @max-[250px]:inline" />
  </span>;
}

function MoreRings({ folded, className }: { folded: PlanUsage[]; className: string }) {
  if (folded.length === 0) return null;
  const low = folded.some((plan) => (lowestWindow(plan)?.remainingPercent ?? 100) <= LOW_PERCENT);
  return <span data-more-plans className={cn("ps-0.5 text-xs tabular-nums", low ? "text-accent-amber" : "text-subtext", className)}>{m.usage_more_plans({ count: folded.length })}</span>;
}

function PlanRing({ plan, agent }: { plan: PlanUsage; agent: Harness | undefined }) {
  const left = lowestWindow(plan)?.remainingPercent;
  return <span data-plan-ring={left === undefined ? "unread" : leftTone(left).text} className="relative grid size-5 place-items-center">
    <Ring size={20} r={8.25} width={1.75} percent={left} stroke={leftTone(left ?? 100).stroke} className="absolute inset-0" />
    <span className={left === undefined ? "opacity-50" : undefined}><PlanMark plan={plan} agent={agent} size="ring" /></span>
  </span>;
}

/** "Usage on <machine>": every plan in one grid, compute by backend, then the last seven days as a graph. */
function UsagePanel({ report, machine, remote, stale, fetchedAt, failure }: {
  report: UsageReport;
  machine: string;
  remote: boolean;
  stale: boolean;
  fetchedAt: number;
  /** Why the latest read failed, above the values it left showing. */
  failure: ReactNode;
}) {
  const workspace = useWorkspace();
  const { data: harnesses = [] } = useQuery(workspace.queries.getHarnessesQuery());
  const planAgent = usePlanAgent();
  const updateAgent = useUpdateAgent(remote);
  const refresh = useMutate(mutRefreshUsage);

  const readAts = (report.plans ?? []).filter((plan) => !plan.error && plan.readAt).map((plan) => Date.parse(plan.readAt!));
  const updatedAt = Math.min(fetchedAt, ...readAts);
  const ai = spendSeries(report, (harness) => harnesses.find((h) => h.id === harness && !h.profile)?.name ?? harness);
  const compute = spendSeries(report);

  const sections = [
    report.plans === null && <LoadingRow key="reading" role="status"><Spinner />{m.usage_reading_plans()}</LoadingRow>,
    report.plans && report.plans.length > 0 && <section key="plans" className={PLAN_GRID}>
      {report.plans.map((plan, index) => <PlanRows key={`${plan.harness}:${plan.profile ?? ""}`} plan={plan} agent={planAgent(plan)} first={index === 0} onUpdate={updateAgent(plan)} />)}
    </section>,
    compute.length > 0 && <ComputeList key="compute" series={compute} />,
    (ai.length > 0 || compute.length > 0) && <SpendGraph key="graph" days={report.days} ai={ai} compute={compute} />,
  ].filter(Boolean);

  return <div className={stale ? "opacity-60" : undefined}>
    <div className="flex items-center justify-between gap-3 pt-1.5">
      <span dir="auto" className="truncate text-xs font-medium text-subtext">{m.usage_on({ machine })}</span>
      <span className="flex shrink-0 items-center gap-0.5 text-xs text-subtext">
        {stale ? m.usage_as_of({ time: fmtClock(fetchedAt) }) : Date.now() - updatedAt < 60_000 ? m.usage_updated_now() : m.usage_updated({ time: timeAgo(updatedAt) })}
        <Tooltip interactive content={m.usage_refresh()}>
          <IconButton size="small" className="-me-1.5" aria-label={m.usage_refresh()} disabled={refresh.isMutating || stale} onClick={() => refresh.run(workspace)}><RotateCw size={12} aria-hidden="true" /></IconButton>
        </Tooltip>
      </span>
    </div>
    {failure}
    {refresh.error != null && <p role="alert" dir="auto" className="mt-1 mb-0 text-xs text-accent-red">{refresh.error instanceof Error ? refresh.error.message : String(refresh.error)}</p>}
    {sections.map((section, index) => <div key={index} className={cn("py-2.5", index > 0 && "border-t border-border-variant")}>{section}</div>)}
  </div>;
}

function ReadFailed({ error, onRetry }: { error: Error; onRetry: () => void }) {
  return <div className="pt-2">
    <div role="alert" className="text-text">{m.usage_plan_failed()}</div>
    <p dir="auto" className="mt-0.5 mb-0 text-xs text-subtext">{error.message}</p>
    <Button size="small" className="mt-2" onClick={onRetry}>{m.usage_try_again()}</Button>
  </div>;
}

/** One agent's or backend's spend in one currency over `UsageReport.days`. */
interface Series {
  key: string;
  name: string;
  backend?: string;
  currency: string;
  days: number[];
  total: number;
  /** Days or runs that cost something no one priced. */
  unpriced: number;
  color: string;
}

const AGENT_COLORS: Record<string, string> = { "claude-code": "bg-accent-orange", codex: "bg-accent-indigo", opencode: "bg-accent-teal", cursor: "bg-accent-blue" };
const BACKEND_COLORS: Record<string, string> = { modal_job: "bg-accent-green", hf_job: "bg-accent-amber", openresearch_job: "bg-accent-pink", vercel_job: "bg-accent-purple", tinker_job: "bg-accent-blue" };
const SPARE_COLORS = ["bg-accent-purple", "bg-accent-pink", "bg-accent-green", "bg-accent-blue", "bg-accent-red", "bg-accent-amber"];

const byTotal = (a: Series, b: Series) => b.total - a.total;
const sum = (values: number[]) => values.reduce((total, value) => total + value, 0);

/** The agents' spend, each named by `agentName` as Settings → Agents does; without it, compute's. */
function spendSeries(report: UsageReport, agentName?: (harness: string) => string): Series[] {
  const rows = agentName
    ? report.agents.map((agent) => ({ ...agent, key: agent.harness, name: agentName(agent.harness), color: AGENT_COLORS[agent.harness], backend: undefined }))
    : report.compute.map((row) => ({ ...row, key: row.backend, name: backendName(row.backend), color: BACKEND_COLORS[row.backend] }));
  return rows.map((row, index): Series => ({
    key: `${row.key}:${row.currency}`,
    name: row.name,
    backend: row.backend,
    currency: row.currency,
    days: row.dayMicros,
    total: sum(row.dayMicros),
    unpriced: row.unpriced,
    color: row.color ?? SPARE_COLORS[index % SPARE_COLORS.length],
  })).filter((series) => series.total > 0 || series.unpriced > 0).sort(byTotal);
}

/** Totals per currency, never converted into one. */
function currencyTotals(series: Series[]) {
  const currencies = [...new Set(series.map((one) => one.currency))];
  return currencies.map((currency) => {
    const shown = series.filter((one) => one.currency === currency);
    return { currency, series: shown, total: sum(shown.map((one) => one.total)) };
  });
}

/** An agent's amount as its bill; a backend's, a rate times running time. */
function amount(series: Series[], micros: number) {
  return (series[0]?.backend ? fmtEstimate : fmtMoney)(micros, series[0]?.currency ?? "USD");
}

/** A backend's amount counts its unpriced runs, so a partial total reads as partial. */
function seriesAmount(series: Series) {
  if (series.total === 0) return m.usage_cost_unavailable();
  const spent = amount([series], series.total);
  if (!series.backend || series.unpriced === 0) return spent;
  const unpriced = new Intl.PluralRules(getLocale()).select(series.unpriced) === "one" ? m.usage_runs_unpriced_one : m.usage_runs_unpriced_other;
  return `${spent} · ${unpriced({ count: series.unpriced })}`;
}

function ComputeList({ series }: { series: Series[] }) {
  const totals = currencyTotals(series).filter((group) => group.total > 0);
  return <section>
    <div className="mb-1 flex items-baseline justify-between gap-3">
      <span className="font-medium text-text">{m.usage_compute_week()}</span>
      <span className="tabular-nums text-text">{totals.map((group) => fmtEstimate(group.total, group.currency)).join(" + ")}</span>
    </div>
    {series.map((one) => <div key={one.key} className="flex items-center justify-between gap-3 py-0.5">
      <span className="flex min-w-0 items-center gap-1.5 text-subtext">{one.backend && <BackendLogo kind={one.backend} size={14} />}<span className="truncate">{one.name}</span></span>
      <span className={cn("shrink-0 tabular-nums", one.total > 0 ? "text-text" : "text-subtext")}>{seriesAmount(one)}</span>
    </div>)}
  </section>;
}

/** The last seven days as stacked daily bars: agents at API prices, or compute. */
function SpendGraph({ days, ai, compute }: { days: string[]; ai: Series[]; compute: Series[] }) {
  const [mode, setMode] = useState<"ai" | "compute">(ai.length > 0 ? "ai" : "compute");
  const shown = mode === "ai" ? ai : compute;
  const labels = days.map((day, index) => {
    if (index === days.length - 1) return m.usage_today();
    const [year, month, date] = day.split("-").map(Number);
    return new Intl.DateTimeFormat(getLocale(), { weekday: "short" }).format(new Date(year, month - 1, date));
  });

  return <section>
    <div className="flex items-center justify-between gap-3">
      <span className="font-medium text-text">{m.usage_last_week()}</span>
      {ai.length > 0 && compute.length > 0 && <div role="group" aria-label={m.usage_graph()} className={cn(SEGMENTED_CLASS_NAME, "[&_button]:h-5 [&_button]:px-2 [&_button]:text-xs")}>
        <button type="button" aria-pressed={mode === "ai"} className={mode === "ai" ? "active" : undefined} onClick={() => setMode("ai")}>{m.usage_graph_ai()}</button>
        <button type="button" aria-pressed={mode === "compute"} className={mode === "compute" ? "active" : undefined} onClick={() => setMode("compute")}>{m.usage_graph_compute()}</button>
      </div>}
    </div>
    {currencyTotals(shown).map((group) => <div key={group.currency} data-spend-graph={mode}>
      <div className="mt-1 flex items-baseline gap-1.5">
        <span className="text-base font-medium tabular-nums text-text">{amount(group.series, group.total)}</span>
        {mode === "ai" && <span className="text-xs text-subtext">{m.usage_at_api_prices()}</span>}
      </div>
      <DayBars series={group.series} labels={labels} />
      <div className="mt-2 flex flex-wrap gap-x-3 gap-y-1 text-xs">
        {group.series.map((one) => <span key={one.key} className="flex items-center gap-1.5 text-subtext">
          <span className={cn("size-2 rounded-[2px]", one.color)} aria-hidden="true" />
          {one.name}
          <span className={cn("tabular-nums", one.total > 0 ? "text-text" : "text-subtext")}>{seriesAmount(one)}</span>
        </span>)}
      </div>
      {mode === "ai" && group.series.some((one) => one.unpriced > 0) && <p className="mt-1.5 mb-0 text-xs text-subtext">{m.usage_unpriced_use()}</p>}
    </div>)}
  </section>;
}

function DayBars({ series, labels }: { series: Series[]; labels: string[] }) {
  const totals = labels.map((_, day) => sum(series.map((one) => one.days[day] ?? 0)));
  const max = Math.max(...totals, 1);
  return <div className="relative mt-2">
    <ul className="sr-only">
      {labels.map((label, day) => totals[day] > 0 && <li key={label}>{m.usage_day_amount({ day: label, amount: amount(series, totals[day]) })}</li>)}
    </ul>
    <div className="flex h-10 items-end gap-1" aria-hidden="true">
      {labels.map((label, day) => <div key={label} className="flex flex-1 flex-col-reverse overflow-hidden rounded-[2px]" style={{ height: `${(totals[day] / max) * 100}%` }}>
        {series.map((one) => (one.days[day] ?? 0) > 0 && <span key={one.key} className={one.color} style={{ height: `${(one.days[day] / totals[day]) * 100}%` }} />)}
      </div>)}
    </div>
    <div className="mt-1 flex gap-1 text-2xs text-subtext" aria-hidden="true">{labels.map((label) => <span key={label} className="flex-1 truncate text-center">{label}</span>)}</div>
  </div>;
}
