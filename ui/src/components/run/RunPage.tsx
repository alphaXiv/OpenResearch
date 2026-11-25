import { useQuery } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { timeAgo, type Run } from "../../api";
import { m } from "../../paraglide/messages.js";
import { runActive, runNotice } from "../../runDisplay";
import { namedStages, runModel, runTone } from "../../runMonitor/model";
import { useWorkspace } from "../../workspace";
import { RunContactNotice } from "../RunContactNotice";
import { harnessLabel } from "../ModelPicker";
import { Button } from "../ui";
import { cn } from "../ui/cn";
import { Block, drawsAt } from "./blocks/Block";
import { Phases } from "./blocks/Phases";
import { Captions } from "./Captions";
import { Issue } from "./Issue";
import { HeadlineStat } from "./HeadlineStat";
import { Lead } from "./Lead";
import { Mark } from "./Mark";
import { Rail } from "./Rail";
import { Sentence } from "./Sentence";
import { useRunMonitors } from "./useRunMonitor";
import { Watch } from "./Watch";

/** The agent of the chat that launched the run, else of the chat that created its experiment, by name; null when neither is known. */
function useChatAgent(run: Run, experimentChat: string | null | undefined): string | null {
  const chat = run.chatSessionId ?? experimentChat;
  const sessions = useQuery({ ...useWorkspace().queries.listChatSessionsQuery(run.projectId), enabled: Boolean(chat) }).data;
  const session = sessions?.find((row) => row.id === chat);
  return session ? harnessLabel(session.harness, session.harnessName) : null;
}

/**
 * A run in full: the sentence, stages, bar and captions, then every block
 * of the view in the agent's order. A failed run can say why even without a monitor.
 */
export function RunPage({ run, name, chatSessionId, now }: { run: Run; name: string; chatSessionId?: string | null; now: number }) {
  const agent = useChatAgent(run, chatSessionId);
  const [monitor] = useRunMonitors([run], () => true);
  if (monitor === null && run.status !== "failed") return null;

  const data = monitor && monitor.status !== "loading" ? monitor.data : null;
  const failed = monitor?.status === "error";
  const model = runModel({
    item: { sessionId: "", run, experimentName: name },
    monitor: data,
    monitorFailed: failed,
    shown: false,
    agent: agent ?? m.chat_the_agent(),
    now,
    nextRetryMs: null,
  });
  const age = model.live && model.sentence.at !== null ? model.sentence.at : null;
  const sentence = model.tone !== "failed" && model.flagDetail ? model.flagDetail : model.sentence.text;
  const blocks = data?.view.blocks.filter((block) => drawsAt(block, "page")) ?? [];
  const incomplete = data?.availability.collectionIncomplete;
  const canSayEmpty = !failed && data?.availability.state === "live" && !incomplete;
  const stale = model.stale && [model.stale.why, model.stale.age].filter(Boolean).join(" · ");
  const empty = !canSayEmpty || blocks.length > 0 ? null
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
            <Sentence text={sentence} tone={model.tone} />
            {age !== null && <span className="whitespace-nowrap text-xs text-ink-subtext"> {timeAgo(age)}</span>}
          </span>
        </p>
      )}
      {stale && !failed && <p className="m-0 mt-1.5 text-xs text-ink-subtext">{stale}</p>}
      {incomplete && <p role="status" className="m-0 mt-1.5 text-sm text-ink-subtext">{m.run_page_collection_incomplete()}</p>}
      {model.note && <p className="m-0 mt-1.5 text-sm text-ink-subtext">{model.note}</p>}
      {model.flag?.record && <Issue model={model} flag={model.flag.record} agent={run.chatSessionId ? agent : null} />}
      {failed && (
        <div className="mt-2 flex items-center gap-2 text-xs text-ink-subtext">
          {stale ?? m.run_monitor_load_failed()}
          <Button size="small" variant="ghost" onClick={monitor.retry}><RefreshCw size={13} />{m.app_retry()}</Button>
        </div>
      )}
      {model.neverStarted ? <p className="m-0 mt-1.5 text-sm text-ink-subtext">{m.run_page_never_started()}</p> : <>
        {!model.pre && data && namedStages(data) && <div className="mt-5"><Phases model={model} phases={data.phases} current={model.live ? data.progress?.phase ?? data.stageOrder.index : data.stageOrder.index} /></div>}
        <div className="mt-3"><Rail model={model} kind="bar" hover /></div>
        <div className="mt-1.5"><Captions model={model} /></div>
      </>}
      {!model.notice && runNotice(run) && <RunContactNotice run={run} className="mt-4" />}
      {model.pre && (model.headline || monitor?.status === "loading") && <div className="mt-7"><HeadlineStat model={model} /></div>}
      {monitor && !model.pre && !model.neverStarted && (!incomplete || blocks.length > 0) && (
        <div className={cn("mt-7 flex flex-col gap-8", !data && "min-h-40")} aria-busy={monitor.status === "loading"}>
          {!data && monitor.status === "loading" && <HeadlineStat model={model} />}
          {data && blocks.map((block, index) => <Block key={index} monitor={data} block={block} size="page" tone={model.tone} live={model.live} />)}
          {empty && <p className="m-0 text-sm text-ink-subtext">{empty}</p>}
        </div>
      )}
    </div>
  );
}

/**
 * The experiment page's Watch section, from the monitor its `RunPage` keeps current, once that has
 * said how the run is watched.
 */
export function WatchSection({ run, chatSessionId, now, className }: { run: Run; chatSessionId?: string | null; now: number; className: string }) {
  const agent = useChatAgent(run, chatSessionId);
  const { getRunMonitorQuery } = useWorkspace().queries;
  // Only what the watch says: the section redraws when a look lands, not with every new point.
  const monitor = useQuery({ ...getRunMonitorQuery(run.id), enabled: false, select: (data) => ({ watch: data.watch, view: data.view, flags: data.flags }) }).data;
  const quiet = monitor?.watch.state === "none" && monitor.watch.looks.length === 0 && !monitor.flags.length;
  if (!monitor || quiet) return null;
  return (
    <section className={className}>
      <h2>{m.run_watch_heading()}</h2>
      <Watch monitor={monitor} runId={run.id} agent={agent} tone={runTone(run)} live={runActive(run.status)} now={now} />
    </section>
  );
}
