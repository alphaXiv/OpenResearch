import { Fragment } from "react";
import { autoDir, fmtNumber, ltr } from "../../../i18n";
import { m } from "../../../paraglide/messages.js";
import { fmtPercent } from "../../../runMonitor/format";
import type { RunMonitor } from "../../../runMonitor/types";
import { cn } from "../../ui/cn";

export function Bars({ monitor, block, size, live }: { monitor: RunMonitor; block: { bars: string; by: string }; size: "strip" | "page"; live: boolean }) {
  const page = size === "page";
  const groups = monitor.groups[block.bars]?.[block.by];
  if (!Array.isArray(groups) || groups.length === 0) {
    const key = autoDir(block.by);
    return <p className="m-0 text-xs text-ink-subtext">{live ? m.run_bars_none({ key }) : m.run_bars_none_ended({ key })}</p>;
  }
  return (
    <>
      <div className={cn("grid items-center", page ? "grid-cols-[minmax(0,auto)_minmax(1rem,1fr)_auto_auto] gap-x-3 gap-y-2 text-sm" : "grid-cols-[auto_1fr_auto] gap-x-2 gap-y-1 text-xs")}>
        {(page ? groups : groups.slice(0, 3)).map(({ key, ok, n }) => {
          const share = n === 0 ? null : ok / n;
          return (
            <Fragment key={key}>
              <span className={cn(page ? "max-w-48 wrap-anywhere text-text" : "max-w-20 truncate text-ink-subtext")}>{autoDir(key)}</span>
              <span aria-hidden="true" className={cn("overflow-hidden rounded-full bg-panel", page ? "h-2" : "h-1.5")}>
                {share !== null && <i className="block h-full rounded-full bg-mark-blue" style={{ width: `${share * 100}%` }} />}
              </span>
              <span className="text-end font-medium tabular-nums text-text">{share === null ? "—" : ltr(fmtPercent(share))}</span>
              {page && <span className="text-end text-xs tabular-nums text-ink-subtext">{m.run_bars_of({ ok: ltr(fmtNumber(ok)), n: ltr(fmtNumber(n)) })}</span>}
            </Fragment>
          );
        })}
      </div>
      {page && <p className="m-0 mt-2.5 text-xs text-ink-subtext">{m.run_bars_retained()}</p>}
    </>
  );
}
