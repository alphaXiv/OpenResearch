import { autoDir } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import type { RunOutcome, RunRule } from "../../runMonitor/types";

/** The rules the run was launched with, which OpenResearch checks with no model; the one that stopped it is marked. */
export function Rules({ rules, outcome, heading }: { rules: RunRule[]; outcome: RunOutcome | null; heading: string }) {
  if (rules.length === 0) return null;
  const stopper = outcome?.by === "rule" ? outcome.rule?.name : undefined;
  return (
    <div>
      <h3 className={heading}>{m.run_rules_heading()}</h3>
      <ul className="m-0 flex list-none flex-col gap-1.5 p-0">
        {rules.map((rule) => (
          <li key={rule.name} className="leading-snug">
            <span className="font-medium">{autoDir(rule.name)}</span>{" "}
            <span>{autoDir(rule.text)}</span>{" "}
            <span className="whitespace-nowrap text-ink-subtext">· {rule.does === "stop" ? m.run_rule_stops() : m.run_rule_asks()}</span>
            {rule.name === stopper && <span className="ms-2 whitespace-nowrap font-medium text-ink-orange">{m.run_rule_stopped_this()}</span>}
          </li>
        ))}
      </ul>
      <p className="m-0 mt-2 text-xs text-ink-subtext">{m.run_rules_when_active()}</p>
    </div>
  );
}
