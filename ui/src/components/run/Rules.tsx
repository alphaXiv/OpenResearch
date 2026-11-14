import { ChevronRight } from "lucide-react";
import { autoDir, fmtNumber, ltr } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { ruleText } from "../../runMonitor/model";
import type { RunOutcome, RunRule } from "../../runMonitor/types";

/** The rules the run was launched with, which OpenResearch checks with no model; the one that stopped it is marked. */
export function Rules({ rules, outcome }: { rules: RunRule[]; outcome: RunOutcome | null }) {
  if (rules.length === 0) return null;
  const summary = new Intl.PluralRules(getLocale()).select(rules.length) === "one" ? m.run_rules_summary_one : m.run_rules_summary_other;
  const stopper = outcome?.by === "rule" ? outcome.rule?.name : undefined;
  return (
    <details className="group mt-3.5 text-sm">
      <summary className="inline-flex cursor-pointer list-none items-center gap-1 rounded-sm text-ink-subtext hover:text-text focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus [&::-webkit-details-marker]:hidden">
        <ChevronRight size={14} aria-hidden="true" className="shrink-0 motion-safe:transition-transform rtl:rotate-180 group-open:rotate-90 rtl:group-open:rotate-90" />
        {summary({ count: ltr(fmtNumber(rules.length)) })}
      </summary>
      <p className="m-0 mt-2 text-xs text-ink-subtext">{m.run_rules_checked()}</p>
      <ul className="m-0 mt-2 flex list-none flex-col gap-1.5 p-0">
        {rules.map((rule) => (
          <li key={rule.name} className="leading-snug">
            <span className="font-medium">{autoDir(rule.name)}</span>{" "}
            <span>{ruleText(rule)}</span>{" "}
            <span className="whitespace-nowrap text-ink-subtext">· {rule.does === "stop" ? m.run_rule_stops() : m.run_rule_asks()}</span>
            {rule.name === stopper && <span className="ms-2 whitespace-nowrap font-medium text-ink-orange">{m.run_rule_stopped_this()}</span>}
          </li>
        ))}
      </ul>
    </details>
  );
}
