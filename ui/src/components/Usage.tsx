import type { ReactNode } from "react";
import { useMutate } from "@clo/react-mutation";
import { useQuery } from "@tanstack/react-query";
import { Info, RotateCw } from "lucide-react";
import type { Harness, PlanResets, PlanUsage, PlanWindow, ResetCredit } from "../api";
import { fmtClock } from "../i18n";
import { harnessDisplayName } from "../modelSelection";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import { mutRefreshUsage } from "../queries/settings";
import { useWorkspace } from "../workspace";
import { BRAND_ICON, AgentIconTile } from "./AgentIcon";
import { HarnessLogo } from "./HarnessLogo";
import { HarnessSetupDialog } from "./HarnessSetupDialog";
import { harnessLabel } from "./ModelPicker";
import { harnessMark } from "./pickerAgents";
import { Button, showAlert, Tooltip } from "./ui";
import { cn } from "./ui/cn";

/** A window at or under this much left is low: amber, and its "left" rides on the composer's trigger. */
export const LOW_PERCENT = 20;
const CRITICAL_PERCENT = 5;

export function lowestWindow(plan: PlanUsage): PlanWindow | undefined {
  return plan.windows.reduce<PlanWindow | undefined>((low, window) => !low || window.remainingPercent < low.remainingPercent ? window : low, undefined);
}

export function leftTone(percent: number) {
  if (percent <= CRITICAL_PERCENT) return { text: "text-accent-red", fill: "bg-accent-red", stroke: "var(--accent-red)" };
  if (percent <= LOW_PERCENT) return { text: "text-accent-amber", fill: "bg-accent-amber", stroke: "var(--accent-amber)" };
  return { text: "text-text", fill: "bg-subtext", stroke: "var(--subtext)" };
}

export const fmtPercent = (percent: number) =>
  new Intl.NumberFormat(getLocale(), { style: "percent", maximumFractionDigits: 0 }).format(Math.floor(percent) / 100);

/** An amount in millionths of `currency`, in the user's locale; never "$0.00" for something that cost a little. */
export function fmtMoney(micros: number, currency: string): string {
  const value = micros / 1_000_000;
  const format = (amount: number) => {
    try {
      return new Intl.NumberFormat(getLocale(), { style: "currency", currency, ...(amount >= 1000 ? { maximumFractionDigits: 0 } : {}) }).format(amount);
    } catch {
      return `${new Intl.NumberFormat(getLocale()).format(amount)} ${currency}`;
    }
  };
  return value > 0 && value < 0.01 ? m.usage_less_than({ amount: format(0.01) }) : format(value);
}

/** Compute's amounts are a rate times running time, never the provider's bill. */
export const fmtEstimate = (micros: number, currency: string) => m.usage_approx({ amount: fmtMoney(micros, currency) });

/** "5-hour", "Weekly", "Fable weekly": named by the window's length, never by the plan. */
export function windowLabel(window: PlanWindow): string {
  const { minutes, model } = window;
  const length = minutes === null ? null
    : minutes === 10080 ? m.usage_window_weekly()
      : minutes % 1440 === 0 ? m.usage_window_days({ count: minutes / 1440 })
        : minutes % 60 === 0 ? m.usage_window_hours({ count: minutes / 60 })
          : m.usage_window_minutes({ count: minutes });
  if (!model) return length ?? m.usage_window_limit();
  return length ? m.usage_window_model({ model, window: length.toLocaleLowerCase(getLocale()) }) : model;
}

/** A time today, a weekday within the week, else a date. */
function fmtReset(ms: number): string {
  const days = (ms - Date.now()) / 86_400_000;
  const sameDay = new Date(ms).toDateString() === new Date().toDateString();
  const options: Intl.DateTimeFormatOptions = sameDay ? { hour: "numeric", minute: "2-digit" } : days < 6 ? { weekday: "short" } : { month: "short", day: "numeric" };
  return new Intl.DateTimeFormat(getLocale(), options).format(ms);
}

const fmtResetFull = (ms: number) =>
  new Intl.DateTimeFormat(getLocale(), { weekday: "long", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }).format(ms);

/** The Settings → Agents entry a plan belongs to: its agent and, for an account OpenResearch keeps, that profile. */
export function usePlanAgent() {
  const { data: harnesses = [] } = useQuery(useWorkspace().queries.getHarnessesQuery());
  return (plan: Pick<PlanUsage, "harness" | "profile">): Harness | undefined =>
    harnesses.find((h) => h.id === plan.harness && (h.profile?.id ?? null) === (plan.profile ?? null));
}

export function planName(plan: PlanUsage, agent: Harness | undefined): string {
  return plan.plan ?? (agent ? harnessDisplayName(agent) : harnessLabel(plan.harness));
}

/** The plan's mark as the model picker draws it: the agent's logo, or a profile's own glyph, emoji or image on its colour. */
export function PlanMark({ plan, agent, size }: { plan: PlanUsage; agent: Harness | undefined; size: "ring" | "xs" }) {
  const mark = agent ? harnessMark(agent) : { icon: BRAND_ICON, brand: (px: number) => <HarnessLogo harness={plan.harness} size={px} /> };
  // Not bare (`outline={false}`): a bare logo grows 4px, past the ring's inner edge.
  return <AgentIconTile icon={mark.icon} brand={mark.brand} size={size} />;
}

/** One line on what the latest read of `plan` says about itself, for a ring's tooltip. */
export function planSummary(plan: PlanUsage, agent: Harness | undefined): string {
  const low = lowestWindow(plan);
  const name = planName(plan, agent);
  const parts: string[] = [low ? m.usage_plan_left({ plan: name, percent: fmtPercent(low.remainingPercent), window: windowLabel(low) }) : m.usage_plan_unread({ plan: name })];
  if (plan.account) parts.push(plan.account);
  return parts.join(" · ");
}

/** Opens the agent's own update for an `unsupported` plan; null where this machine can't run it from here. */
export function useUpdateAgent(remote: boolean) {
  const workspace = useWorkspace();
  const { getHarnessesQuery, getHarnessSetupCommandsQuery } = workspace.queries;
  const { data: harnesses = [] } = useQuery(getHarnessesQuery());
  const { data: commands } = useQuery({ ...getHarnessSetupCommandsQuery(), enabled: !remote });
  return (plan: PlanUsage) => {
    const harness = harnesses.find((h) => h.id === plan.harness && !h.profile);
    const setup = commands?.[plan.harness];
    if (remote || !harness || !setup) return null;
    return () => workspace.dialogs.showCustomDialog({
      component: HarnessSetupDialog,
      props: { harness, commands: setup, action: "update" },
      onConfirm: () => {},
      onCancel: "close",
    });
  };
}

/** A plan row's way forward, read as a link that keeps its 24px target. */
const ROW_ACTION = "-ms-1.5 h-6 px-1.5 text-xs font-normal text-text underline underline-offset-2";

/** The columns `PlanRows` fill: label, bar, what's left, reset. Plans sharing one grid line their bars up. */
export const PLAN_GRID = "grid grid-cols-[max-content_minmax(2.5rem,1fr)_auto_auto] items-center gap-x-2.5 gap-y-1.5";

/** A plan's heading and a row per window, as children of `PLAN_GRID`. */
export function PlanRows({ plan, agent, first, onUpdate }: {
  plan: PlanUsage;
  agent: Harness | undefined;
  first: boolean;
  onUpdate: (() => void) | null;
}) {
  const workspace = useWorkspace();
  const retry = useMutate(mutRefreshUsage);
  const failed = plan.error?.kind === "failed";
  const agentName = harnessLabel(plan.harness);

  return <>
    <div className={cn("col-span-4 flex min-w-0 items-center gap-1.5", !first && "mt-1.5")}>
      <PlanMark plan={plan} agent={agent} size="xs" />
      <span className="shrink-0 font-medium text-text">{planName(plan, agent)}</span>
      {plan.account && <span dir="auto" className="truncate text-subtext">{plan.account}</span>}
    </div>
    {plan.windows.map((window, index) => <WindowRow key={index} window={window} dimmed={failed} />)}
    {plan.credits && (plan.credits.unlimited || plan.credits.balance !== null) && <DetailRow label={m.usage_credits()} dimmed={failed}>
      {plan.credits.unlimited ? m.usage_credits_unlimited() : plan.credits.balance}
    </DetailRow>}
    {plan.extraUsage && <DetailRow label={m.usage_extra()} dimmed={failed}>
      {plan.extraUsage.limitMicros === null
        ? fmtMoney(plan.extraUsage.usedMicros, plan.extraUsage.currency)
        : m.usage_extra_of({ used: fmtMoney(plan.extraUsage.usedMicros, plan.extraUsage.currency), limit: fmtMoney(plan.extraUsage.limitMicros, plan.extraUsage.currency) })}
    </DetailRow>}
    {plan.resets && <ResetRow plan={plan} resets={plan.resets} />}
    {plan.error?.kind === "unsupported" && <div className="col-span-4 flex flex-wrap items-center gap-x-1 text-xs text-subtext">
      <span>{plan.error.version ? m.usage_plan_unsupported({ agent: agentName, version: plan.error.version }) : m.usage_plan_unsupported_no_version({ agent: agentName })}</span>
      {onUpdate && <Button size="small" variant="ghost" className={ROW_ACTION} onClick={onUpdate}>{m.usage_update_agent({ agent: agentName })}</Button>}
    </div>}
    {plan.error?.kind === "failed" && <div className="col-span-4 flex flex-wrap items-center gap-x-1 text-xs text-subtext">
      <span>{m.usage_plan_failed()}</span>
      {plan.windows.length > 0 && plan.readAt && <span>{m.usage_as_of({ time: fmtClock(Date.parse(plan.readAt)) })}</span>}
      <Tooltip content={plan.error.message}><Info size={12} aria-hidden="true" /></Tooltip>
      <Button size="small" variant="ghost" className={ROW_ACTION} disabled={retry.isMutating} onClick={() => retry.run(workspace)}>{m.usage_try_again()}</Button>
      {retry.error != null && <p role="alert" dir="auto" className="m-0 basis-full text-accent-red">{retry.error instanceof Error ? retry.error.message : String(retry.error)}</p>}
    </div>}
  </>;
}

/** The listed credit that expires first, which is the one to spend. */
function soonestCredit(resets: PlanResets): ResetCredit | undefined {
  const expiry = (credit: ResetCredit) => credit.expiresAt ? Date.parse(credit.expiresAt) : Infinity;
  return [...resets.credits ?? []].sort((a, b) => expiry(a) - expiry(b))[0];
}

const fmtDate = (ms: number) => new Intl.DateTimeFormat(getLocale(), { month: "short", day: "numeric" }).format(ms);

/** A plan's resets and Use…, which confirms against the plan's lowest window before spending one. */
function ResetRow({ plan, resets }: { plan: PlanUsage; resets: PlanResets }) {
  const workspace = useWorkspace();
  const credit = soonestCredit(resets);
  const name = resets.available === 1 && credit?.title
    ? credit.title
    : (new Intl.PluralRules(getLocale()).select(resets.available) === "one" ? m.usage_reset_count_one : m.usage_reset_count_other)({ count: resets.available });
  const expires = credit?.expiresAt ? Date.parse(credit.expiresAt) : NaN;

  // A press in the dialog closes the popover around this row, so every outcome is said in the dialog or a toast.
  const confirm = () => {
    const low = lowestWindow(plan);
    // One key per attempt, sent again when a failed attempt is retried, so it never spends two resets.
    let attempt: string | null = null;
    workspace.dialogs.showConfirmDialog({
      title: credit?.title ? m.usage_reset_title_named({ reset: credit.title }) : m.usage_reset_title(),
      description: <>
        {credit?.description && <span dir="auto" className="mb-2 block">{credit.description}</span>}
        <span className="block">{m.usage_reset_effect()}</span>
        {low && <span className="mt-2 block text-text tabular-nums">{m.usage_reset_lowest({ window: windowLabel(low), percent: fmtPercent(low.remainingPercent) })}</span>}
      </>,
      confirmLabel: m.usage_reset_confirm(),
      onCancel: "close",
      onConfirm: async () => {
        attempt ??= crypto.randomUUID();
        const outcome = await workspace.api.resetUsage({ profile: plan.profile, creditId: credit?.id, attempt });
        attempt = null;
        if (outcome === "nothingToReset") throw new Error(m.usage_reset_nothing());
        if (outcome === "noCredit") throw new Error(m.usage_reset_none_left());
        showAlert(m.usage_reset_done(), "success");
      },
    });
  };

  return <div data-plan-resets className="col-span-4 flex flex-wrap items-center gap-x-1 text-xs text-subtext">
    <span dir="auto">{Number.isFinite(expires) ? m.usage_reset_use_by({ reset: name, date: fmtDate(expires) }) : name}</span>
    <Button size="small" variant="ghost" className={ROW_ACTION} aria-label={m.usage_reset_use_named({ reset: name })} onClick={confirm}>{m.usage_reset_confirm()}</Button>
  </div>;
}

function WindowRow({ window, dimmed }: { window: PlanWindow; dimmed: boolean }) {
  const tone = leftTone(window.remainingPercent);
  const resets = window.resetsAt ? Date.parse(window.resetsAt) : NaN;
  const dim = dimmed && "opacity-55";
  return <>
    <span className={cn("whitespace-nowrap text-subtext", dim)}>{windowLabel(window)}</span>
    <span aria-hidden="true" className={cn("relative h-[5px] overflow-hidden rounded-full bg-border", dim)}>
      <span className={cn("absolute inset-y-0 start-0 rounded-full", tone.fill)} style={{ width: `${Math.max(0, Math.min(100, window.remainingPercent))}%` }} />
    </span>
    <span className={cn("whitespace-nowrap text-end tabular-nums", tone.text, dim)}>{m.usage_percent_left({ percent: fmtPercent(window.remainingPercent) })}</span>
    {Number.isFinite(resets)
      ? <Tooltip content={m.usage_resets({ date: fmtResetFull(resets) })} className={cn("justify-self-end rounded-sm", dim)}>
        <span className="flex items-center gap-1 whitespace-nowrap text-xs tabular-nums text-subtext"><RotateCw size={10} aria-hidden="true" />{fmtReset(resets)}</span>
      </Tooltip>
      : <span />}
  </>;
}

function DetailRow({ label, dimmed, children }: { label: string; dimmed: boolean; children: ReactNode }) {
  const dim = dimmed && "opacity-55";
  return <>
    <span className={cn("whitespace-nowrap text-subtext", dim)}>{label}</span>
    <span className={cn("col-span-3 text-end tabular-nums text-text", dim)}>{children}</span>
  </>;
}

/** A track and an arc filling `percent` of it clockwise from the top; without `percent`, a dotted track. */
export function Ring({ size, r, width, percent, stroke, className }: {
  size: number;
  r: number;
  width: number;
  percent?: number;
  stroke: string;
  className?: string;
}) {
  const c = 2 * Math.PI * r;
  const center = size / 2;
  return <svg viewBox={`0 0 ${size} ${size}`} width={size} height={size} className={className} aria-hidden="true">
    <circle cx={center} cy={center} r={r} fill="none" stroke="var(--border)" strokeWidth={width} strokeDasharray={percent === undefined ? "2 2.4" : undefined} />
    {percent !== undefined && <circle cx={center} cy={center} r={r} fill="none" stroke={stroke} strokeWidth={width} strokeLinecap="round"
      strokeDasharray={`${(c * Math.max(percent, 2)) / 100} ${c}`} transform={`rotate(-90 ${center} ${center})`} />}
  </svg>;
}

/** The gauge: usage before there is a ring to show. */
export function UsageGauge() {
  return <svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" className="shrink-0" aria-hidden="true">
    <path d="M2.5 11.5a5.5 5.5 0 1 1 11 0" />
    <path d="M8 11.5 10.6 7.4" />
  </svg>;
}
