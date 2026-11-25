import { Popover } from "@base-ui/react/popover";
import { ArrowUpRight, RefreshCw } from "lucide-react";
import { useContext, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { backendDetail, backendKind, fmtNumber, type Run } from "../../api";
import { localMachineFacts } from "../../localMachine";
import { useWorkspace } from "../../workspace";
import { BackendLogo, backendName } from "../BackendLogos";
import { autoDir, fmtClock, ltr } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import { plainSentence } from "../../runMonitor/format";
import type { RunModel } from "../../runMonitor/model";
import { monitorBehind } from "../../runMonitor/revision";
import { foldedStages } from "../../runMonitor/stages";
import { capitalized, progressWords } from "../../runMonitor/words";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../../tabPreview";
import { WorkspaceDomContext } from "../../WorkspaceDom";
import { RunDiffStat } from "../RunDiffStat";
import { EscapeScope } from "../EscapeScope";
import { Button } from "../ui";
import { cn } from "../ui/cn";
import { Phases } from "./blocks/Phases";
import { Lead } from "./Lead";
import { Sentence } from "./Sentence";
import { useRunMonitors } from "./useRunMonitor";

export function RunStatus({ model, label, descriptionId, onOpenRunPage, expanded = false }: {
  model: RunModel;
  label?: string;
  descriptionId?: string;
  onOpenRunPage?: (run: Run, intent: TabOpenIntent) => void;
  expanded?: boolean;
}) {
  const { portalContainer } = useContext(WorkspaceDomContext);
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const { getLocalMachineQuery, listExperimentsQuery } = useWorkspace().queries;
  const kind = backendKind(model.run.backend);
  const local = useQuery({ ...getLocalMachineQuery(), enabled: open && kind === "local_job", subscribed: open && kind === "local_job" });
  const experiments = useQuery({ ...listExperimentsQuery(model.run.projectId), enabled: open, subscribed: open });
  const experiment = experiments.data?.find((item) => item.id === model.run.experimentId);
  const parent = experiments.data?.find((item) => item.id === experiment?.parentExperimentId);
  const detail = backendDetail(model.run.backend);
  const facts = kind === "local_job" && local.data && !local.isError ? localMachineFacts(local.data) : null;
  const host = kind === "ssh_job" || kind === "slurm_job" ? model.run.host : null;
  const diffRunId = experiment?.parentExperimentId && model.run.commitSha ? model.run.id : null;
  const monitor = useRunMonitors([model.run], () => open)[0];
  const data = monitor && monitor.status !== "loading" ? monitor.data : null;
  const behind = data && monitorBehind(model.run.monitor, data);
  const stages = data && !behind && !model.neverStarted ? foldedStages(data, model) : null;
  const progress = data?.progress;
  const availability = data?.availability;
  const availabilityNote = availability && availability.state !== "live"
    ? availability.reason ?? (!model.live && availability.state === "stale" ? null : m.run_view_stale())
    : null;
  const count = data && progress?.done != null ? autoDir(plainSentence(capitalized(progressWords(data.phases[progress.phase], progress.done)))) : "";
  const time = [model.eta, model.etaDetail].filter(Boolean).join(" · ")
    || (!model.live ? [model.lead, model.sentence.text].filter(Boolean).join(" · ") : "");
  const untimed = [...(data?.estimate?.untimed ?? model.run.monitor?.estimate?.untimed ?? [])].sort((a, b) => b.length - a.length);
  const footer = autoDir(model.settingUp || model.etaStage || !model.live ? time : m.run_whole_time({ time: autoDir(time) }));
  const footerNames = untimed.length ? new RegExp(`(${untimed.map((name) => name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "g") : null;
  const stage = model.sentence.stage;
  const wrapWarning = expanded && model.tone === "flag";
  const wrapSentence = wrapWarning || model.notice !== null;
  const sentence = wrapWarning && !stage && model.sentence.watcher
    ? model.flagDetail ?? model.sentence.text
    : model.sentence.text;
  const status = plainSentence([label, model.lead, sentence].filter(Boolean).join(" · "));
  const cardSentence = model.tone === "failed" ? model.sentence.text : model.sentence.watcher;

  // Focus returns before closing so it cannot reopen the focus-triggered popover.
  function close() {
    trigger.current?.focus({ preventScroll: true });
    setOpen(false);
  }

  function onKeyDown(event: KeyboardEvent) {
    if (event.key !== "Escape" || !open) return;
    event.preventDefault();
    event.stopPropagation();
    close();
  }

  return (
    <EscapeScope enabled={open} onEscape={close}>
      <Popover.Root open={open} onOpenChange={setOpen}>
        <Popover.Trigger ref={trigger} openOnHover delay={180} closeDelay={180}
          onFocus={(event) => { if (event.currentTarget.matches(":focus-visible")) setOpen(true); }}
          onKeyDown={onKeyDown}
          aria-label={m.run_status_for({ status: autoDir(status), name: autoDir(model.name) })}
          aria-describedby={descriptionId}
          className="flex min-h-6 min-w-0 flex-1 items-center overflow-hidden rounded-xs text-start whitespace-nowrap @max-[30rem]:block @max-[30rem]:py-1 @max-[30rem]:whitespace-normal @max-[30rem]:wrap-break-word @max-[30rem]:[&_span]:whitespace-normal focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2"
        >
          {label && <span className="me-1 min-w-0 shrink-[0.001] truncate text-ink-subtext @max-[30rem]:me-0">{autoDir(label)} · </span>}
          <Lead model={model} />
          {stage ? <>
            {stage.name && <span className="min-w-0 shrink truncate"><Sentence text={stage.name} tone={model.tone} /></span>}
            {stage.count && <span className={cn("shrink-0", stage.name && "ms-1")}><Sentence text={stage.count} tone={model.tone} /></span>}
            {model.sentence.watcher && <span className="min-w-0 flex-1 basis-0 truncate"><Sentence text={m.run_stage_with_look({ stage: "", sentence: autoDir(model.sentence.watcher) })} tone={model.tone} /></span>}
          </> : <span className={cn("min-w-0", wrapSentence ? "wrap-anywhere whitespace-normal" : "truncate")}><Sentence text={sentence} tone={model.tone} /></span>}
        </Popover.Trigger>
        <Popover.Portal container={portalContainer}>
          <Popover.Positioner side="top" align="start" anchor={() => trigger.current?.closest<HTMLElement>("[data-run-frame]") ?? trigger.current} sideOffset={6} collisionPadding={12} className="z-50">
            <Popover.Popup initialFocus={false} finalFocus={false} onKeyDown={onKeyDown}
              className="w-96 max-w-[calc(100vw-1.5rem)] max-h-(--available-height) overflow-y-auto rounded-lg border border-border bg-background px-4 py-3.5 text-text shadow-menu outline-none"
            >
              <Popover.Title className="m-0 text-sm font-medium">
                {onOpenRunPage ? <Button variant="ghost" role="link" className="tool-target min-h-6 w-full justify-start gap-1 whitespace-normal p-0 text-start text-sm font-medium"
                  {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => { close(); onOpenRunPage(model.run, intent); })}
                >{autoDir(model.name)}<ArrowUpRight size={13} className="shrink-0" aria-hidden="true" /></Button> : autoDir(model.name)}
              </Popover.Title>
              {kind && <Popover.Description className="m-0 mt-2.5 flex min-w-0 items-center gap-2 text-xs">
                <BackendLogo kind={kind} size={14} />
                <span className="shrink-0 font-medium">{backendName(kind)}</span>
                {detail && <span title={detail} className="min-w-0 truncate text-ink-subtext">{autoDir(detail)}</span>}
              </Popover.Description>}
              <dl className="m-0 mt-1.5 grid grid-cols-[5.25rem_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs empty:hidden">
                {host && <Fact label={m.run_card_host()} title={host}>{autoDir(host)}</Fact>}
                {facts && <>
                  <Fact label={m.run_card_processor()} title={facts.processor}>{autoDir(facts.processor)}</Fact>
                  {facts.cores !== null && <Fact label={m.run_card_cores()}>{fmtNumber(facts.cores)}</Fact>}
                  {facts.memory && <Fact label={m.run_card_memory()}>{facts.memory}</Fact>}
                  {facts.gpus > 0 && <Fact label={m.run_card_gpus()}>{fmtNumber(facts.gpus)}</Fact>}
                </>}
              </dl>
              <dl className="m-0 mt-2.5 grid grid-cols-[5.25rem_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs empty:hidden">
                {parent && <Fact label={m.run_card_from()} title={parent.slug}>{autoDir(parent.slug)}</Fact>}
                <RunDiffStat runId={diffRunId} parentSlug={parent?.slug ?? null} wrap={(stat) => <Fact label={m.run_card_changes()}>{stat}</Fact>} />
                {model.run.startedAt != null && <Fact label={m.run_card_started()}>{ltr(fmtClock(model.run.startedAt))}</Fact>}
              </dl>
              {monitor?.status === "error" ? <div className="mt-2.5 text-xs text-ink-subtext" role="alert">
                {m.run_monitor_load_failed()} <Button size="small" variant="ghost" onClick={monitor.retry}><RefreshCw size={13} />{m.app_retry()}</Button>
              </div> : monitor?.status === "loading" || behind ? <p className="m-0 mt-2.5 text-xs text-ink-subtext" role="status">{m.common_loading()}</p> : data && <>
                {availabilityNote && <p className="m-0 mt-2.5 text-xs text-ink-subtext">{autoDir(availabilityNote)}</p>}
                {data.availability.state !== "unavailable" && stages && <div className="mt-2.5 border-t border-border-variant pt-2.5">
                  <Phases model={model} phases={stages.phases} current={stages.current} progress={count} setupNames={stages.names} className="flex-col gap-y-2" />
                </div>}
              </>}
              {cardSentence && <p className="m-0 mt-2.5 break-words text-sm"><Sentence text={autoDir(cardSentence)} tone={model.tone} /></p>}
              {time && <p className="m-0 mt-2.5 border-t border-border-variant pt-2.5 text-xs text-ink-subtext">{footerNames ? footer.split(footerNames).map((part, index) => untimed.includes(part) ? <span key={index} className="whitespace-nowrap">{part}</span> : part) : <Sentence text={footer} tone={model.tone} />}</p>}
            </Popover.Popup>
          </Popover.Positioner>
        </Popover.Portal>
      </Popover.Root>
    </EscapeScope>
  );
}

function Fact({ label, title, children }: { label: string; title?: string; children: ReactNode }) {
  return <>
    <dt className="text-ink-subtext">{label}</dt>
    <dd title={title} className="m-0 truncate text-text">{children}</dd>
  </>;
}
