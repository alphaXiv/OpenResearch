import { useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  decideChatSleepStrategy,
  getChatSleep,
  startChatSleep,
  stopChatSleep,
  turnOffChatSleep,
  type SleepStatus,
} from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { getLocale } from "../paraglide/runtime.js";
import { Button, Input, Switch } from "./ui";
import { useDialogFocus } from "./useDialogFocus";

export const chatSleepQuery = (sessionId: string) => ({
  queryKey: ["chat-sleep", sessionId] as const,
  queryFn: ({ signal }: { signal: AbortSignal }) => getChatSleep(sessionId, signal),
  refetchInterval: 30_000,
});

const fmtUsd = (value: number) =>
  new Intl.NumberFormat(getLocale(), { style: "currency", currency: "USD" }).format(value);

const fmtPct = (value: number) =>
  new Intl.NumberFormat(getLocale(), { maximumFractionDigits: 1, signDisplay: "always" }).format(value) + "%";

/** The pill detail in the model picker: off, on with spend, or finished. */
export function sleepLabel(sleep: SleepStatus | null | undefined) {
  if (!sleep) return m.sleep_mode_off();
  if (sleep.state.finished) return m.sleep_mode_finished();
  return `${m.sleep_mode_on()} · ${fmtUsd(sleep.budget.spentUsd)} / ${fmtUsd(sleep.budget.limitUsd)}`;
}

const optionalNumber = (text: string) => {
  const trimmed = text.trim();
  if (!trimmed) return null;
  const value = Number(trimmed);
  return Number.isFinite(value) ? value : Number.NaN;
};

export function SleepModeDialog({ sessionId, onClose }: { sessionId: string; onClose: () => void }) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const queryClient = useQueryClient();
  const { data: sleep } = useQuery(chatSleepQuery(sessionId));
  const plan = sleep?.plan;
  const objective = plan?.objective;
  const hoursLeft = plan?.until ? Math.max(0, (plan.until - Date.now()) / 3_600_000) : null;
  const [budget, setBudget] = useState(plan ? String(plan.gpuBudgetUsd) : "10");
  const [hours, setHours] = useState(hoursLeft != null ? hoursLeft.toFixed(1) : "8");
  const [goal, setGoal] = useState(plan?.goal ?? "");
  const [metric, setMetric] = useState(objective?.metric ?? "");
  const [direction, setDirection] = useState<"max" | "min">(objective?.direction ?? "max");
  const [target, setTarget] = useState(objective?.improvePct != null ? String(objective.improvePct) : "");
  const [baseline, setBaseline] = useState(
    objective?.baselineValue != null ? String(objective.baselineValue) : "",
  );
  const [selfImprove, setSelfImprove] = useState(plan?.selfImprove ?? true);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const active = !!sleep && !sleep.state.finished;

  useDialogFocus(dialogRef, onClose);

  const run = async (action: () => Promise<SleepStatus | null>, close: boolean) => {
    setSaving(true);
    setError(null);
    try {
      const next = await action();
      queryClient.setQueryData(chatSleepQuery(sessionId).queryKey, next);
      if (close) onClose();
    } catch (cause) {
      setError(m.sleep_mode_failed({ error: cause instanceof Error ? cause.message : String(cause) }));
    } finally {
      setSaving(false);
    }
  };

  const start = () => {
    const budgetUsd = Number(budget);
    const hoursValue = optionalNumber(hours);
    const targetPct = optionalNumber(target);
    const baselineValue = optionalNumber(baseline);
    if (!budget.trim() || !Number.isFinite(budgetUsd) || budgetUsd < 0) {
      setError(m.sleep_mode_invalid_budget());
      return;
    }
    if ([hoursValue, targetPct, baselineValue].some((value) => Number.isNaN(value))) {
      setError(m.sleep_mode_invalid_number());
      return;
    }
    void run(
      () =>
        startChatSleep(sessionId, {
          budgetUsd,
          hours: hoursValue,
          goal: goal.trim() || null,
          metric: metric.trim() || null,
          direction,
          targetPct,
          baselineValue,
          selfImprove,
        }),
      true,
    );
  };

  const progress = sleep?.progress;
  const strategy = sleep?.strategy;
  const record = (stats: { wins: number; losses: number } | null | undefined) =>
    m.sleep_mode_strategy_record({ wins: ltr(String(stats?.wins ?? 0)), judged: ltr(String((stats?.wins ?? 0) + (stats?.losses ?? 0))) });

  return createPortal(
    <div
      className="fixed inset-0 z-200 flex items-center justify-center bg-modal-backdrop p-5"
      onClick={(event) => {
        if (!saving && event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="sleep-mode-dialog max-h-full w-120 max-w-full overflow-y-auto rounded-xl border border-border bg-background p-6 shadow-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="sleep-mode-dialog-title"
        tabIndex={-1}
      >
        <h2 id="sleep-mode-dialog-title" className="m-0 text-xl font-medium text-text">
          {m.sleep_mode_title()}
        </h2>
        <p className="mt-2 mb-0 text-sm text-muted">{m.sleep_mode_description()}</p>

        {sleep && (
          <div className="mt-4 space-y-1 rounded-md border border-border-variant bg-surface p-3 text-sm text-text">
            <p className="m-0 font-medium">{sleepLabel(sleep)}</p>
            <p className="m-0">
              {m.sleep_mode_spent({
                spent: ltr(fmtUsd(sleep.budget.spentUsd)),
                limit: ltr(fmtUsd(sleep.budget.limitUsd)),
              })}
            </p>
            {progress && (progress.baseline && progress.best ? (
              <p className="m-0">
                {m.sleep_mode_progress({
                  best: ltr(String(progress.best.value)),
                  baseline: ltr(String(progress.baseline.value)),
                  pct: ltr(fmtPct(progress.improvementPct ?? 0)),
                })}
              </p>
            ) : (
              <p className="m-0 text-muted">{m.sleep_mode_no_baseline()}</p>
            ))}
            {sleep.state.assessment && (
              <p className="m-0">
                {m.sleep_mode_verdict({ verdict: ltr(sleep.state.assessment.verdict) })}{" "}
                <span className="text-muted">{sleep.state.assessment.note}</span>
              </p>
            )}
            {strategy && (
              <div className="sleep-mode-strategy mt-2 space-y-1 border-t border-border-variant pt-2">
                <p className="m-0">
                  {m.sleep_mode_strategy_in_force({ version: ltr(String(strategy.incumbent.version)) })}{" "}
                  <span className="text-muted">{record(strategy.incumbent.stats)}</span>
                </p>
                {strategy.candidate && (
                  <p className="m-0">
                    {m.sleep_mode_strategy_trial({
                      version: ltr(String(strategy.candidate.version)),
                      candidate: record(strategy.candidate.stats),
                      control: record(strategy.candidate.control),
                    })}{" "}
                    <span className="text-muted">{strategy.candidate.rationale}</span>
                  </p>
                )}
                {!strategy.measurable && strategy.selfImprove && (
                  <p className="m-0 text-muted">{m.sleep_mode_strategy_needs_metric()}</p>
                )}
                {strategy.events.slice(0, 3).map((event) => (
                  <p key={`${event.at}-${event.kind}`} className="m-0 truncate text-muted" title={event.detail}>
                    v{event.version} {event.kind.replace("_", " ")}: {event.detail}
                  </p>
                ))}
                <div className="flex flex-wrap gap-2 pt-1">
                  {strategy.candidate && (
                    <>
                      <Button size="small" disabled={saving} onClick={() => void run(() => decideChatSleepStrategy(sessionId, "promote"), false)}>
                        {m.sleep_mode_strategy_promote()}
                      </Button>
                      <Button size="small" disabled={saving} onClick={() => void run(() => decideChatSleepStrategy(sessionId, "reject"), false)}>
                        {m.sleep_mode_strategy_reject()}
                      </Button>
                    </>
                  )}
                  {strategy.incumbent.parent != null && (
                    <Button size="small" disabled={saving} onClick={() => void run(() => decideChatSleepStrategy(sessionId, "rollback"), false)}>
                      {m.sleep_mode_strategy_rollback()}
                    </Button>
                  )}
                </div>
              </div>
            )}
            {sleep.state.finished && <p className="m-0 text-muted">{sleep.state.finished.detail}</p>}
            {sleep.chatPrompts && !sleep.state.finished && (
              <p className="m-0 text-accent-amber">{m.sleep_mode_prompts_warning()}</p>
            )}
          </div>
        )}

        <div className="mt-4 grid grid-cols-2 gap-3 text-sm text-text">
          <label className="flex flex-col gap-1">
            {m.sleep_mode_budget()}
            <Input data-initial-focus inputMode="decimal" value={budget} onChange={(e) => setBudget(e.target.value)} />
          </label>
          <label className="flex flex-col gap-1">
            {m.sleep_mode_hours()}
            <Input inputMode="decimal" value={hours} onChange={(e) => setHours(e.target.value)} />
          </label>
          <label className="col-span-2 flex flex-col gap-1">
            {m.sleep_mode_goal()}
            <textarea
              className="min-h-16 w-full resize-y rounded-md border border-border bg-background px-2.5 py-1.5 font-sans text-sm text-text outline-none placeholder:text-muted focus:border-text"
              rows={2}
              maxLength={2000}
              value={goal}
              placeholder={m.sleep_mode_goal_placeholder()}
              onChange={(e) => setGoal(e.target.value)}
            />
            <span className="text-muted">{m.sleep_mode_goal_hint()}</span>
          </label>
          <label className="col-span-2 flex flex-col gap-1">
            {m.sleep_mode_metric()}
            <Input
              value={metric}
              placeholder={m.sleep_mode_metric_placeholder()}
              onChange={(e) => setMetric(e.target.value)}
            />
          </label>
          {metric.trim() && (
            <>
              <div className="col-span-2 flex gap-2" role="group" aria-label={m.sleep_mode_direction()}>
                <Button size="small" variant={direction === "max" ? "primary" : "default"} aria-pressed={direction === "max"} onClick={() => setDirection("max")}>
                  {m.sleep_mode_direction_max()}
                </Button>
                <Button size="small" variant={direction === "min" ? "primary" : "default"} aria-pressed={direction === "min"} onClick={() => setDirection("min")}>
                  {m.sleep_mode_direction_min()}
                </Button>
              </div>
              <label className="flex flex-col gap-1">
                {m.sleep_mode_target()}
                <Input inputMode="decimal" value={target} placeholder="15" onChange={(e) => setTarget(e.target.value)} />
              </label>
              <label className="flex flex-col gap-1">
                {m.sleep_mode_baseline()}
                <Input inputMode="decimal" value={baseline} onChange={(e) => setBaseline(e.target.value)} />
              </label>
            </>
          )}
          <label className="col-span-2 flex items-start gap-2.5">
            <Switch
              checked={selfImprove}
              aria-label={m.sleep_mode_self_improve()}
              onClick={() => setSelfImprove((value) => !value)}
            />
            <span className="flex flex-col gap-0.5">
              {m.sleep_mode_self_improve()}
              <span className="text-muted">{m.sleep_mode_self_improve_hint()}</span>
            </span>
          </label>
        </div>

        {error && <p className="mt-3 mb-0 text-sm text-accent-red">{error}</p>}

        <div className="mt-6 flex flex-wrap justify-end gap-2.5">
          {sleep && (
            <Button variant="danger" disabled={saving} onClick={() => void run(() => turnOffChatSleep(sessionId), true)}>
              {m.sleep_mode_turn_off()}
            </Button>
          )}
          {active && (
            <Button disabled={saving} onClick={() => void run(() => stopChatSleep(sessionId), false)}>
              {m.sleep_mode_stop()}
            </Button>
          )}
          <Button disabled={saving} onClick={onClose}>{m.settings_page_cancel()}</Button>
          <Button variant="primary" disabled={saving} onClick={start}>
            {active ? m.sleep_mode_update() : m.sleep_mode_start()}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
