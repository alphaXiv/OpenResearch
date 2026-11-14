import { Fragment } from "react";
import { autoDir, fmtNumber, ltr } from "../../../i18n";
import { m } from "../../../paraglide/messages.js";
import { fmtPercent } from "../../../runMonitor/format";
import type { RunMonitor } from "../../../runMonitor/types";
import { cn } from "../../ui/cn";

/** A signal by a key, as the share of each key's items that counted: `ok` of `n`. A strip has room for three keys. */
export function Bars({ monitor, block, size, live }: { monitor: RunMonitor; block: { bars: string; by: string }; size: "strip" | "page"; live: boolean }) {
  const page = size === "page";
  const groups = (monitor.groups[block.bars] ?? []).slice(0, page ? undefined : 3);
  if (groups.length === 0) {
    const key = autoDir(block.by);
    return <p className="m-0 text-xs text-ink-subtext">{live ? m.run_bars_none({ key }) : m.run_bars_none_ended({ key })}</p>;
  }
  return (
    <div className={cn("grid items-center", page ? "grid-cols-[auto_1fr_auto_auto] gap-x-3 gap-y-2 text-sm" : "grid-cols-[auto_1fr_auto] gap-x-2 gap-y-1 text-xs")}>
      {groups.map((group) => {
        const share = group.value === null ? null : Math.min(1, Math.max(0, group.value));
        return (
          <Fragment key={group.key}>
            <span className={cn("truncate", page ? "text-text" : "max-w-20 text-ink-subtext")}>{autoDir(group.key)}</span>
            <span aria-hidden="true" className={cn("overflow-hidden rounded-full bg-panel", page ? "h-2" : "h-1.5")}>
              {share !== null && <i className="block h-full rounded-full bg-mark-blue" style={{ width: `${share * 100}%` }} />}
            </span>
            <span className="text-end font-medium tabular-nums text-text">{share === null ? "—" : ltr(fmtPercent(share))}</span>
            {page && <span className="text-end text-xs tabular-nums text-ink-subtext">{m.run_bars_of({ ok: ltr(fmtNumber(group.ok)), n: ltr(fmtNumber(group.n)) })}</span>}
          </Fragment>
        );
      })}
    </div>
  );
}
