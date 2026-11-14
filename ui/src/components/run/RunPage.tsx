import { useQuery } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { timeAgo, type Run } from "../../api";
import { hostForRun } from "../../engineHosts";
import { m } from "../../paraglide/messages.js";
import { runActive, runNotice } from "../../runDisplay";
import { namedStages, runModel, runTone } from "../../runMonitor/model";
import { useWorkspace } from "../../workspace";
import { RunContactNotice } from "../RunContactNotice";
import { Button } from "../ui";
import { cn } from "../ui/cn";
import { Block, drawsAt } from "./blocks/Block";
import { Phases } from "./blocks/Phases";
import { Captions } from "./Captions";
import { Lead } from "./Lead";
import { Mark } from "./Mark";
import { Rail } from "./Rail";
import { Rules } from "./Rules";
import { Sentence } from "./Sentence";
import { useRunMonitors } from "./useRunMonitor";
import { ViewSource } from "./ViewSource";
import { Watch } from "./Watch";

/**
 * A run in full, in its experiment's Latest run: the sentence, the stages, the bar and its captions, then every block
 * of the view in the agent's order. Nothing against an older server, which keeps the run's status and log.
 */
export function RunPage({ run, name, now }: { run: Run; name: string; now: number }) {
  const hosts = useWorkspace().queries.useEngineHosts();
  const [monitor] = useRunMonitors([run], () => true);
  if (monitor === null) return null;

  const data = monitor.status === "loading" ? null : monitor.data;
  const failed = monitor.status === "error";
  const host = hostForRun(hosts, run);
  const model = runModel({
    item: { sessionId: "", run, experimentName: name },
    monitor: data,
    monitorFailed: failed,
    shown: false,
    agent: m.chat_the_agent(),
    now,
    nextRetryMs: host?.status === "backoff" ? host.nextRetryMs : null,
  });
  const age = model.live && model.sentence.at !== null ? model.sentence.at : null;
  const blocks = data?.view.blocks.filter((block) => drawsAt(block, "page")) ?? [];
  const empty = !data || blocks.length > 0 ? null
    : !data.viewFrom.isDefault ? m.run_page_view_empty()
    : model.live ? m.run_page_no_signals()
    : m.run_page_no_signals_ended();

  return (
    <div role="group" aria-label={name} className="mt-5">
      {model.notice ? <RunContactNotice run={run} /> : (
        <p className="m-0 flex items-baseline gap-2 text-base leading-relaxed">
          <span className="flex h-[1.625em] shrink-0 items-center self-start"><Mark tone={model.tone} live={model.live} label={model.status} /></span>
          <span>
            <Lead model={model} stop />
            <Sentence text={model.flagDetail ?? model.sentence.text} tone={model.tone} />
            {age !== null && <span className="whitespace-nowrap text-xs text-ink-subtext"> {timeAgo(age)}</span>}
          </span>
        </p>
      )}
      {model.stale && !failed && <p className="m-0 mt-1.5 text-xs text-ink-subtext">{model.stale.why} · {model.stale.age}</p>}
      {model.note && <p className="m-0 mt-1.5 text-sm text-ink-subtext">{model.note}</p>}
      {failed && (
        <div className="mt-2 flex items-center gap-2 text-xs text-ink-subtext">
          {model.stale ? `${model.stale.why} · ${model.stale.age}` : m.run_monitor_load_failed()}
          <Button size="small" variant="ghost" onClick={monitor.retry}><RefreshCw size={13} />{m.app_retry()}</Button>
        </div>
      )}
      {!model.pre && data && namedStages(data) && <div className="mt-5"><Phases model={model} monitor={data} /></div>}
      <div className="mt-3"><Rail model={model} kind="bar" hover /></div>
      <div className="mt-1.5"><Captions model={model} /></div>
      {!model.notice && runNotice(run) && <RunContactNotice run={run} className="mt-4" />}
      {!model.pre && (
        <div className={cn("mt-7 flex flex-col gap-8", !data && "min-h-40")} aria-busy={monitor.status === "loading"}>
          {data && blocks.map((block, index) => <Block key={index} monitor={data} block={block} size="page" projectId={run.projectId} tone={model.tone} live={model.live} />)}
          {empty && <p className="m-0 text-sm text-ink-subtext">{empty}</p>}
        </div>
      )}
      {data && <Rules rules={data.rules} outcome={run.outcome ?? null} />}
      {data && <ViewSource monitor={data} projectId={run.projectId} live={model.live} />}
    </div>
  );
}

/**
 * The experiment page's Watch section for its latest run, from the monitor its `RunPage` keeps current, once that has
 * said how the run is watched.
 */
export function WatchSection({ run, now, className }: { run: Run; now: number; className: string }) {
  const { getRunMonitorQuery } = useWorkspace().queries;
  // Only what the watch says: the section redraws when a look lands, not with every new point.
  const monitor = useQuery({ ...getRunMonitorQuery(run.id), enabled: false, select: (data) => ({ watch: data.watch, view: data.view, brief: data.brief }) }).data;
  // Shown where there is something to read or do: looks, or watching the user turned off. A run whose chat has
  // no watching model reads its stage and progress as its sentence, and there is no model to pick here yet.
  const quiet = monitor && ["none", "needs_pick", "no_model"].includes(monitor.watch.state) && monitor.watch.looks.length === 0;
  if (!monitor || quiet) return null;
  return (
    <section className={className}>
      <h2>{m.run_watch_heading()}</h2>
      <Watch monitor={monitor} runId={run.id} tone={runTone(run)} live={runActive(run.status)} now={now} />
    </section>
  );
}
