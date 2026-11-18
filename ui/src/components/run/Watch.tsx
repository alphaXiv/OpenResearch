import { useMutate } from "@clo/react-mutation";
import { ChevronRight, RefreshCw, TriangleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { LookTooSoonError } from "../../api";
import { autoDir, fmtNumber, ltr } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { mutLookNow } from "../../queries/projects";
import { flagLook, flagOutcome, flagRow, journalRows, listed, lookText, type JournalRow } from "../../runMonitor/journal";
import { TONE_INK, type Tone } from "../../runMonitor/model";
import type { Flag, RunMonitor } from "../../runMonitor/types";
import { capitalized, clockText } from "../../runMonitor/words";
import { useWorkspace } from "../../workspace";
import { Button, Tooltip } from "../ui";
import { cn } from "../ui/cn";
import { Sentence } from "./Sentence";

/** A look's cost, which is often a fraction of a cent: never rounded to nothing. */
const usd = (value: number) => new Intl.NumberFormat(getLocale(), {
  style: "currency",
  currency: "USD",
  ...(value < 1 ? { maximumSignificantDigits: 2 } : {}),
}).format(value);

/**
 * The page's Watch section: one line per change in what the watcher said, each opening to its whole text and how
 * it came about, then how many looks it took and what they cost, and Look now. A run nobody watches says why.
 */
export function Watch({ monitor, runId, agent, tone, live, now }: { monitor: Pick<RunMonitor, "watch" | "view" | "flags">; runId: string; agent: string | null; tone: Tone; live: boolean; now: number }) {
  const workspace = useWorkspace();
  const look = useMutate(mutLookNow);
  const [refused, setRefused] = useState<string | LookTooSoonError | null>(null);
  const { watch } = monitor;
  const looks = watch.looks.filter((look) => look.said !== "error" && look.said !== "interrupted");
  const flags = monitor.flags;
  // A flag a tripwire or a rule raised stands in the journal beside the looks, in time order.
  const rows = journalRows([...watch.looks, ...flags.filter((flag) => !flag.lookId).map(flagLook)].sort((a, b) => a.at - b.at));
  const lastLook = watch.looks.at(-1)?.id;
  useEffect(() => setRefused(null), [runId, lastLook]);

  const floorUntil = Math.max(watch.floorUntil ?? 0, refused instanceof LookTooSoonError ? refused.notBefore : 0);
  const wait = floorUntil > now ? m.run_watch_look_too_soon({ clock: clockText(floorUntil) }) : null;
  const refusal = refused instanceof LookTooSoonError ? wait : refused;
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
        <ol aria-label={m.run_watch_journal()} className="m-0 grid list-none grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-0.5 p-0 text-sm">
          {rows.map((row, index) => (
            <Entry
              key={row.since?.id ?? row.look.id}
              row={row}
              latest={index === 0}
              raised={flags.find((candidate) => flagRow(candidate) === row.look.id) ?? null}
              monitor={monitor}
              agent={agent}
              tone={tone}
            />
          ))}
        </ol>
      )}
      {(totals || watched) && (
        <div className="mt-3.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-ink-subtext">
          {totals && <span className="tabular-nums">{totals}</span>}
          {watched && live && (
            <Tooltip interactive disabled={!wait} content={wait ?? ""}>
              <Button
                size="small"
                variant="ghost"
                className="-my-1"
                aria-disabled={Boolean(wait) || look.isPending}
                aria-description={wait ?? undefined}
                onClick={() => {
                  if (wait || look.isPending) return;
                  setRefused(null);
                  look.runWithOptions(workspace, runId, {
                    onError: (error) => setRefused(error instanceof LookTooSoonError ? error : error instanceof Error ? error.message : String(error)),
                  });
                }}
              >
                <RefreshCw size={13} className={cn(look.isPending && "motion-safe:animate-spin")} />{look.isPending ? m.run_watch_looking() : m.run_watch_look_now()}
              </Button>
            </Tooltip>
          )}
          {refusal && <span role="status" className={TONE_INK.flag}>{refusal}</span>}
        </div>
      )}
    </div>
  );
}

/** One change in the journal: its time and words on one line, which open to the whole text and how it came about. */
function Entry({ row, latest, raised, monitor, agent, tone }: {
  row: JournalRow;
  latest: boolean;
  raised: Flag | null;
  monitor: Pick<RunMonitor, "view">;
  agent: string | null;
  tone: Tone;
}) {
  const [open, setOpen] = useState(false);
  const { look } = row;
  const failed = look.said === "error" || look.said === "interrupted";
  const flag = look.said === "flag";
  const said = lookText(look, monitor.view.signals) || autoDir(look.withheld[0] ?? "");
  const outcome = raised ? flagOutcome(raised, agent) : null;
  const why = look.said === "interrupted" ? m.run_watch_interrupted() : failed && look.text ? autoDir(look.text) : null;
  const detail = [
    why,
    outcome && `${outcome}${raised?.choice?.reason ? ` ${autoDir(raised.choice.reason)}` : ""}`,
    look.notedUnder && m.run_watch_noted({ clock: clockText(look.notedUnder) }),
    row.since && capitalized(m.run_watch_same_since({ count: ltr(fmtNumber(row.count)), clock: clockText(row.since.at) })),
    look.with.length > 0 && m.run_watch_with({ names: listed(look.with) }),
    look.omitted && autoDir(look.omitted),
  ].filter((line): line is string => Boolean(line));

  return (
    <li className="col-span-2 grid grid-cols-subgrid">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((was) => !was)}
        className="relative col-span-2 -mx-1.5 grid min-h-6 grid-cols-subgrid rounded-sm px-1.5 py-1 text-start hover:bg-surface focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus"
      >
        <span className="whitespace-nowrap pt-0.5 text-xs tabular-nums text-ink-subtext">{clockText(look.at)}</span>
        <span className={cn("min-w-0 pe-5 leading-snug", open ? "wrap-anywhere" : "truncate", latest && !failed || flag ? "text-text" : "text-ink-subtext")}>
          {flag && <TriangleAlert size={13} role="img" aria-label={m.run_watch_flagged()} className="me-1.5 inline-block align-[-2px] text-mark-amber" />}
          {look.withheld.length > 0 && <TriangleAlert size={13} role="img" aria-label={listed(look.withheld)} className="me-1.5 inline-block align-[-2px] text-mark-amber" />}
          {failed || !latest && !flag ? said.replaceAll("**", "") : <Sentence text={said} tone={flag ? "flag" : tone} />}
          <ChevronRight size={13} aria-hidden="true" className={cn("absolute end-1.5 top-1.5 text-ink-subtext rtl:rotate-180", open && "rotate-90 rtl:rotate-90")} />
        </span>
      </button>
      {open && (detail.length > 0 || look.withheld.length > 0) && (
        <div className="col-start-2 flex min-w-0 flex-col gap-0.5 pb-1.5 text-xs wrap-anywhere text-ink-subtext">
          {look.withheld.map((note) => <span key={note} className={TONE_INK.flag}>{autoDir(note)}</span>)}
          {detail.map((line) => <span key={line}>{line}</span>)}
        </div>
      )}
    </li>
  );
}
