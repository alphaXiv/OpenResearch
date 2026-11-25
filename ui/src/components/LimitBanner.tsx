import { useEffect, useState } from "react";
import { useMutate } from "@clo/react-mutation";
import { Gauge } from "lucide-react";
import { fmtDuration, type PlanResets, type PlanUsage } from "../api";
import { fmtClock } from "../i18n";
import { m } from "../paraglide/messages.js";
import { mutSetChatLimitTimer } from "../queries/chat";
import { useWorkspace } from "../workspace";
import { useSpendReset } from "./Usage";
import { Button } from "./ui";

/** Docked above the composer while the chat's account is out of usage, or while a stopped
 * timer still holds its queue after the reset. Without `sessionId` it speaks for a new chat. */
export function LimitBanner({ agent, resetAt, sessionId, stopped, queued, plan }: {
  agent: string;
  /** Epoch ms; null once the account has usage again. */
  resetAt: number | null;
  sessionId?: string;
  stopped: boolean;
  queued: number;
  plan?: PlanUsage;
}) {
  const workspace = useWorkspace();
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (resetAt === null) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [resetAt]);
  const timer = useMutate(mutSetChatLimitTimer);

  if (resetAt === null && !(stopped && queued > 0)) return null;
  const wait = resetAt !== null && resetAt > now ? fmtDuration(resetAt - now) : null;
  const time = resetAt === null ? "" : fmtClock(resetAt);
  const detail = resetAt === null ? m.chat_limit_back_waits()
    : !sessionId ? m.chat_limit_new_chat({ time })
    : stopped ? m.chat_limit_waits({ time })
    : m.chat_limit_continues({ time });
  const toggle = sessionId && (resetAt === null
    ? { label: m.chat_limit_send_now(), stop: false }
    : { label: stopped ? m.chat_limit_start_timer() : m.chat_limit_stop_timer(), stop: !stopped });
  const resets = resetAt !== null && plan?.resets && plan.resets.available > 0 ? plan.resets : undefined;

  return (
    <div role="status" className="limit-banner mb-2.5 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border border-border bg-accent-amber-subtle px-3 py-2 text-sm text-text">
      <Gauge size={16} className="shrink-0 text-accent-amber" aria-hidden="true" />
      <div className="min-w-0 flex-1">
        <p className="m-0 font-medium">{resetAt === null ? m.chat_limit_back_title({ agent }) : m.chat_limit_title({ agent })}</p>
        <p className="m-0 text-subtext">{detail}</p>
        {timer.error !== undefined && (
          <p role="alert" dir="auto" className="m-0 text-accent-red">{timer.error instanceof Error ? timer.error.message : String(timer.error)}</p>
        )}
      </div>
      {(wait || resets || toggle) && (
        <div className="flex shrink-0 flex-wrap items-center gap-2">
          {wait && <span className="tabular-nums text-subtext">{wait}</span>}
          {resets && plan && <UseReset plan={plan} resets={resets} />}
          {toggle && (
            <Button type="button" size="small" disabled={timer.isPending} onClick={() => timer.run(workspace, sessionId, toggle.stop)}>
              {toggle.label}
            </Button>
          )}
        </div>
      )}
    </div>
  );
}

function UseReset({ plan, resets }: { plan: PlanUsage; resets: PlanResets }) {
  const spend = useSpendReset(plan, resets);
  return <Button type="button" size="small" variant="primary" onClick={spend}>{m.usage_reset_confirm()}</Button>;
}
