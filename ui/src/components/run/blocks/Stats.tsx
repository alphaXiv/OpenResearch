import { fmtNumber, ltr } from "../../../i18n";
import { m } from "../../../paraglide/messages.js";
import { getLocale } from "../../../paraglide/runtime.js";
import { fmtMagnitude, fmtSignal } from "../../../runMonitor/format";
import { versusOf } from "../../../runMonitor/model";
import type { RunMonitor, VersusBy } from "../../../runMonitor/types";
import { Delta } from "../Delta";
import { Spark } from "./Spark";

/** A label as it reads after another word: lowercased, unless it is an acronym such as "FID". */
export const midSentence = (label: string) => /^\p{Lu}{2}/u.test(label) ? label : label.charAt(0).toLocaleLowerCase(getLocale()) + label.slice(1);

/**
 * Each signal's latest value, or its best where the view asks and the server says which that is, with its course and
 * how the latest compares with the parent, the block's way or else each signal's own. A page block only.
 */
export function Stats({ monitor, names, vs: by, live }: { monitor: RunMonitor; names: string[]; vs?: VersusBy; live: boolean }) {
  return (
    <dl className="m-0 grid grid-cols-[repeat(auto-fit,minmax(9.5rem,1fr))] gap-x-8 gap-y-4">
      {names.map((name) => {
        const spec = monitor.view.signals[name];
        const label = spec?.label ?? name;
        const best = spec?.pick === "best" ? monitor.best?.[name] : undefined;
        const vs = best !== undefined ? null : versusOf(monitor, name, live, by);
        const unit = spec?.unit && spec.unit !== "%" ? spec.unit : null;
        return (
          <div key={name} className="min-w-0">
            <dt className="truncate text-xs text-ink-subtext">{best === undefined ? label : m.run_stat_best({ label: midSentence(label) })}</dt>
            <dd className="m-0 mt-0.5 flex items-center gap-2">
              <span className="text-xl font-semibold leading-tight tabular-nums text-text">{ltr(fmtSignal(spec, best ?? monitor.latest[name]))}</span>
              {unit && <span className="text-xs text-ink-subtext">{unit}</span>}
              <span className="ms-auto"><Spark points={monitor.series[name] ?? []} /></span>
            </dd>
            {vs && (
              <dd className="m-0 mt-1 text-xs text-ink-subtext">
                {vs.flat ? m.run_level_with_parent() : (
                  <>
                    <Delta delta={{ down: vs.d < 0, good: vs.good, flat: false, magnitude: ltr(fmtMagnitude(spec, vs.mag)) }} said />
                    {" "}{vs.step === undefined ? m.run_than_parent_ended() : m.run_than_parent_at_step({ step: ltr(fmtNumber(vs.step)) })}
                  </>
                )}
              </dd>
            )}
          </div>
        );
      })}
    </dl>
  );
}
