import { useMutate } from "@clo/react-mutation";
import { ChevronRight, RefreshCw, TriangleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { autoDir, fmtNumber, ltr } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { mutLookNow } from "../../queries/projects";
import { flagLook, flagOutcome, flagRow, journalRows, listed, lookText } from "../../runMonitor/journal";
import { TONE_INK, type Tone } from "../../runMonitor/model";
import type { BriefLabel, RunMonitor } from "../../runMonitor/types";
import { capitalized, clockText } from "../../runMonitor/words";
import { useWorkspace } from "../../workspace";
import { Button, Tooltip } from "../ui";
import { cn } from "../ui/cn";
import { Sentence } from "./Sentence";

const BRIEF: Record<BriefLabel, () => string> = {
  question: m.run_brief_question,
  healthy: m.run_brief_healthy,
  noise: m.run_brief_noise,
  worth_a_flag: m.run_brief_worth_a_flag,
  from_parent: m.run_brief_from_parent,
  decided: m.run_brief_decided,
};

/** A look's cost, which is often a fraction of a cent: never rounded to nothing. */
const usd = (value: number) => new Intl.NumberFormat(getLocale(), {
  style: "currency",
  currency: "USD",
  ...(value < 1 ? { maximumSignificantDigits: 2 } : {}),
}).format(value);

/**
 * The page's Watch section: what the watcher said of this run, one row per change, how many looks it took and what
 * they cost, and Look now. A run nobody watches says why, in place of the journal.
 */
export function Watch({ monitor, runId, tone, live, now }: { monitor: Pick<RunMonitor, "watch" | "view" | "brief" | "flags">; runId: string; tone: Tone; live: boolean; now: number }) {
  const workspace = useWorkspace();
  const look = useMutate(mutLookNow);
  const [refused, setRefused] = useState<string | null>(null);
  const { watch } = monitor;
  const signals = monitor.view.signals;
  const looks = watch.looks.filter((look) => look.said !== "error" && look.said !== "interrupted");
  const flags = monitor.flags ?? [];
  // A flag a tripwire or a rule raised stands in the journal beside the looks, in time order.
  const rows = journalRows([...watch.looks, ...flags.filter((flag) => !flag.lookId).map(flagLook)].sort((a, b) => a.at - b.at));
  const agent = capitalized(m.chat_the_agent());
  const lastLook = watch.looks.at(-1)?.id;
  useEffect(() => setRefused(null), [lastLook]);

  const tooSoon = watch.floorUntil !== null && watch.floorUntil !== undefined && watch.floorUntil > now;
  const watched = watch.state === "on";
  const absent = watch.state === "off" ? m.run_watch_off()
    : watch.state === "needs_pick" ? m.run_watch_needs_pick()
    : watch.state === "no_model" ? m.run_watch_no_model({ why: autoDir(watch.why ?? "") })
    : watch.state === "unknown" ? watch.error?.message ?? m.run_watch_unknown()
    : null;
  const totals = looks.length === 0 || !watch.model ? null
    : (new Intl.PluralRules(getLocale()).select(looks.length) === "one" ? m.run_watch_looks_one : m.run_watch_looks_other)({
      count: ltr(fmtNumber(looks.length)),
      model: autoDir(watch.model),
      cost: watch.spentUsd === null ? m.run_watch_cost_unknown() : ltr(usd(watch.spentUsd)),
    });

  return (
    <div>
      {absent && <p className="m-0 text-sm text-ink-subtext">{absent}</p>}
      {!absent && rows.length === 0 && (
        <p className="m-0 text-sm text-ink-subtext">{live ? m.run_watch_first_look() : m.run_watch_never_looked()}</p>
      )}
      {rows.length > 0 && (
        <ol aria-label={m.run_watch_journal()} className="m-0 grid list-none grid-cols-[auto_1fr] gap-x-4 gap-y-2.5 p-0 text-sm">
          {rows.map((row, index) => {
            const failed = row.look.said === "error" || row.look.said === "interrupted";
            const flag = row.look.said === "flag";
            const raised = flags.find((candidate) => flagRow(candidate) === row.look.id);
            const outcome = raised ? flagOutcome(raised, agent) : null;
            return (
              <li key={row.look.id} className="contents">
                <span className="whitespace-nowrap pt-0.5 text-xs tabular-nums text-ink-subtext">{clockText(row.look.at)}</span>
                <span className={cn("leading-snug", index === 0 && !failed || flag ? "text-text" : "text-ink-subtext")}>
                  {flag && <TriangleAlert size={13} aria-hidden="true" className="me-1.5 inline-block align-[-2px] text-mark-amber" />}
                  {flag && <span className="font-medium">{raised && !raised.lookId ? m.run_watch_flagged_orx() : m.run_watch_flagged()} </span>}
                  {failed || (index > 0 && !flag)
                    ? lookText(row.look, signals).replaceAll("**", "")
                    : <Sentence text={lookText(row.look, signals)} tone={flag ? "flag" : tone} />}
                  {outcome && (
                    <span className="block text-xs text-ink-subtext">
                      {outcome}{raised?.choice?.reason ? ` ${autoDir(raised.choice.reason)}` : ""}
                    </span>
                  )}
                  {row.look.withheld?.map((note) => (
                    <span key={note} className={cn("block text-xs", TONE_INK.flag)}>{autoDir(note)}</span>
                  ))}
                  {row.since && (
                    <span className="block text-xs text-ink-subtext">{capitalized(m.run_watch_same_since({ count: ltr(fmtNumber(row.count)), clock: clockText(row.since.at) }))}</span>
                  )}
                  {row.look.with.length > 0 && (
                    <span className="block text-xs text-ink-subtext">{m.run_watch_with({ names: listed(row.look.with) })}</span>
                  )}
                </span>
              </li>
            );
          })}
        </ol>
      )}
      {(totals || watched) && (
        <div className="mt-3.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-ink-subtext">
          {totals && <span className="tabular-nums">{totals}</span>}
          {watched && live && (
            tooSoon ? (
              <Tooltip interactive content={m.run_watch_look_too_soon({ clock: clockText(watch.floorUntil!) })}>
                <Button size="small" variant="ghost" className="-my-1" aria-disabled="true" aria-description={m.run_watch_look_too_soon({ clock: clockText(watch.floorUntil!) })}>
                  <RefreshCw size={13} />{m.run_watch_look_now()}
                </Button>
              </Tooltip>
            ) : (
              <Button
                size="small"
                variant="ghost"
                className="-my-1"
                disabled={look.isPending}
                onClick={() => {
                  setRefused(null);
                  look.runWithOptions(workspace, runId, { onError: (error) => setRefused(error instanceof Error ? error.message : String(error)) });
                }}
              >
                <RefreshCw size={13} className={cn(look.isPending && "motion-safe:animate-spin")} />{look.isPending ? m.run_watch_looking() : m.run_watch_look_now()}
              </Button>
            )
          )}
          {refused && <span role="status" className={TONE_INK.flag}>{refused}</span>}
        </div>
      )}
      {monitor.brief && monitor.brief.length > 0 && (
        <details className="group mt-3.5 text-sm">
          <summary className="inline-flex cursor-pointer list-none items-center gap-1 rounded-sm text-ink-subtext hover:text-text focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus [&::-webkit-details-marker]:hidden">
            <ChevronRight size={14} aria-hidden="true" className="shrink-0 motion-safe:transition-transform rtl:rotate-180 group-open:rotate-90 rtl:group-open:rotate-90" />
            {m.run_watch_told()}
          </summary>
          <dl className="m-0 mt-2.5 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5">
            {monitor.brief.filter((line) => Object.hasOwn(BRIEF, line.label)).map((line) => (
              <div key={line.label} className="contents">
                <dt className="text-xs text-ink-subtext">{BRIEF[line.label]()}</dt>
                <dd className="m-0 text-text">{autoDir(line.text)}</dd>
              </div>
            ))}
          </dl>
        </details>
      )}
    </div>
  );
}
