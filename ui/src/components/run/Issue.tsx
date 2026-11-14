import { useQuery } from "@tanstack/react-query";
import { CircleStop } from "lucide-react";
import { useId, type MouseEvent } from "react";
import { autoDir, fmtNumber, ltr } from "../../i18n";
import type { LogSeek } from "../../logWindow";
import { m } from "../../paraglide/messages.js";
import { chooseFlag, type FlagAnswer } from "../../queries/projects";
import type { RunModel } from "../../runMonitor/model";
import type { Flag } from "../../runMonitor/types";
import { clockText } from "../../runMonitor/words";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../../tabPreview";
import { useWorkspace } from "../../workspace";
import { Button } from "../ui";

/** A flag's evidence, and whose move it is. What the agent decides is said in the chat, not here. */
export function Issue({ model, flag, agent, onOpenRun }: {
  model: RunModel;
  flag: Flag;
  agent: string;
  onOpenRun?: (runId: string, intent: TabOpenIntent, seek?: LogSeek) => void;
}) {
  const workspace = useWorkspace();
  const choice = useQuery(workspace.queries.flagChoiceQuery(model.run.id, flag.id)).data;
  const evidenceId = useId();
  const { evidence } = flag;
  const state = model.flag?.state;
  const name = autoDir(agent);
  const where = evidence && (evidence.from === null || evidence.to === null ? m.chat_run_logs()
    : evidence.from === evidence.to ? m.run_flag_log_lines_one({ from: ltr(fmtNumber(evidence.from)) })
    : m.run_flag_log_lines_other({ from: ltr(fmtNumber(evidence.from)), to: ltr(fmtNumber(evidence.to)) }));
  const status = model.looking || state === "asking"
    ? (evidence ? m.run_flag_reading({ agent: name }) : m.run_flag_reading_what({ agent: name }))
    : state === "raised" || state === "waking" ? m.run_flag_waking({ agent: name }) : null;

  const locked = Boolean(choice?.sending);
  const pick = (value: FlagAnswer) => ({
    "aria-disabled": locked || undefined,
    className: "aria-disabled:cursor-default aria-disabled:opacity-45",
    onClick: (event: MouseEvent<HTMLButtonElement>) => {
      const button = event.currentTarget;
      chooseFlag(workspace, model.run, flag.id, value, () => {
        if (button.isConnected) button.focus();
      });
    },
  });

  return (
    <div className="mt-2.5">
      {evidence && (
        <div className="rounded-md border border-border-variant bg-background">
          <pre id={evidenceId} dir="ltr" className="m-0 overflow-x-auto px-2.5 pt-2 pb-1.5 font-mono text-xs leading-relaxed text-text">{evidence.lines.join("\n")}</pre>
          <div className="px-2.5 pb-1.5 text-xs text-ink-subtext">
            {onOpenRun
              ? <button className="tool-target" {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpenRun(model.run.id, intent, { at: evidence.byteFrom, ident: evidence.ident }))}>{where}</button>
              : where}
          </div>
        </div>
      )}
      {flag.noted?.map((noted) => (
        <p key={noted.lookId} className="m-0 mt-1.5 text-xs text-ink-subtext">
          {m.run_flag_also_noted({ clock: clockText(noted.at), text: autoDir(noted.text.replaceAll("**", "")) })}
        </p>
      ))}
      {model.needsChoice ? (
        <div className="mt-2.5 flex flex-wrap items-center gap-2" aria-describedby={evidence ? evidenceId : undefined}>
          <Button size="small" variant="danger" {...pick("stop")}><CircleStop size={14} />{m.experiments_stop_run()}</Button>
          <Button size="small" {...pick("keep")}>{m.run_flag_keep_going()}</Button>
          <Button size="small" {...pick("ask")}>{m.run_flag_ask({ agent: name })}</Button>
          <span className="ms-auto text-xs text-ink-subtext">{m.run_flag_until_you_choose()}</span>
          {!locked && choice?.error && <p role="alert" className="m-0 w-full text-xs text-ink-red">{choice.error}</p>}
        </div>
      ) : status && (
        <p className="m-0 mt-2 text-sm"><span className={state === "raised" || state === "waking" ? "text-ink-subtext" : "tool-running-shimmer"}>{status}</span></p>
      )}
    </div>
  );
}
