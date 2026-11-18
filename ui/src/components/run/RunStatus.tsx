import { Popover } from "@base-ui/react/popover";
import { ArrowUpRight, RefreshCw } from "lucide-react";
import { useContext, useRef, useState, type KeyboardEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import { backendDetail, backendKind, type Run } from "../../api";
import { localMachineSummary } from "../../localMachine";
import { useWorkspace } from "../../workspace";
import { BackendLogo, backendName } from "../BackendLogos";
import { autoDir, fmtClock, ltr } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import { plainSentence } from "../../runMonitor/format";
import { monitorBehind, type RunModel } from "../../runMonitor/model";
import { foldedStages } from "../../runMonitor/stages";
import { capitalized, progressWords } from "../../runMonitor/words";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../../tabPreview";
import { WorkspaceDomContext } from "../../WorkspaceDom";
import { RunDiffStat } from "../RunDiffStat";
import { EscapeScope } from "../EscapeScope";
import { Button } from "../ui";
import { Phases } from "./blocks/Phases";
import { Lead } from "./Lead";
import { Sentence } from "./Sentence";
import { useRunMonitors } from "./useRunMonitor";

export function RunStatus({ model, label, descriptionId, onOpenRunPage }: {
  model: RunModel;
  label?: string;
  descriptionId?: string;
  onOpenRunPage?: (run: Run, intent: TabOpenIntent) => void;
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
  const hardware = kind === "local_job" && local.data && !local.isError ? localMachineSummary(local.data) : null;
  const compute = [...new Set([kind === "ssh_job" || kind === "slurm_job" ? model.run.host : null, detail, hardware].filter(Boolean))].map((part) => autoDir(String(part))).join(" · ");
  const monitor = useRunMonitors([model.run], () => open)[0];
  const data = monitor && monitor.status !== "loading" ? monitor.data : null;
  const behind = data && monitorBehind(model.run.monitor, data);
  const stages = data && !behind ? foldedStages(data, model) : null;
  const progress = data?.progress;
  const count = data && progress?.done != null ? plainSentence(capitalized(progressWords(data.phases[progress.phase], progress.done))) : "";
  const time = [model.eta, model.etaDetail].filter(Boolean).join(" · ") || (!model.live ? model.sentence.text : "");
  const untimed = [...(data?.estimate?.untimed ?? model.run.monitor?.estimate?.untimed ?? [])].sort((a, b) => b.length - a.length);
  const footer = model.settingUp || !model.live ? time : m.run_whole_time({ time });
  const footerNames = untimed.length ? new RegExp(`(${untimed.map((name) => name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "g") : null;
  const stage = model.sentence.stage;
  const status = plainSentence([label, model.lead, model.sentence.text].filter(Boolean).join(" · "));

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
          aria-label={m.run_status_for({ status, name: autoDir(model.name) })}
          aria-describedby={descriptionId}
          className="flex min-h-6 min-w-0 flex-1 items-center overflow-hidden whitespace-nowrap rounded-xs text-start focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2"
        >
          {label && <span className="me-1 min-w-0 shrink truncate text-ink-subtext">{autoDir(label)} · </span>}
          <Lead model={model} />
          {stage ? <>
            <span className="min-w-0 shrink truncate"><Sentence text={stage.name} tone={model.tone} /></span>
            {stage.count && <span className="ms-1 shrink-0"><span aria-hidden="true">· </span><Sentence text={stage.count} tone={model.tone} /></span>}
            {model.sentence.watcher && <span className="min-w-0 flex-1 basis-0 truncate"><Sentence text={m.run_stage_with_look({ stage: "", sentence: autoDir(model.sentence.watcher) })} tone={model.tone} /></span>}
          </> : <span className="min-w-0 truncate"><Sentence text={model.sentence.text} tone={model.tone} /></span>}
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
              {kind && <Popover.Description className="m-0 mt-1.5 flex items-start gap-2 text-xs text-ink-subtext">
                <span className="shrink-0"><BackendLogo kind={kind} size={14} /></span>
                <span>{backendName(kind)}{compute && <> · {compute}</>}</span>
              </Popover.Description>}
              {(parent || model.run.startedAt != null || experiment?.parentExperimentId && model.run.commitSha) && <div className="mt-1.5 flex flex-wrap items-baseline gap-x-1 gap-y-1 text-xs text-ink-subtext [&>*+*]:before:me-1 [&>*+*]:before:content-['·']">
                {parent && <span>{m.exp_hover_card_from()} {autoDir(parent.slug)}</span>}
                <RunDiffStat runId={experiment?.parentExperimentId && model.run.commitSha ? model.run.id : null} parentSlug={parent?.slug ?? null} />
                {model.run.startedAt != null && <span>{m.history_started({ time: ltr(fmtClock(model.run.startedAt)) })}</span>}
              </div>}
              {monitor?.status === "error" ? <div className="mt-2.5 text-xs text-ink-subtext" role="alert">
                {m.run_monitor_load_failed()} <Button size="small" variant="ghost" onClick={monitor.retry}><RefreshCw size={13} />{m.app_retry()}</Button>
              </div> : monitor?.status === "loading" || behind ? <p className="m-0 mt-2.5 text-xs text-ink-subtext" role="status">{m.common_loading()}</p> : data && <>
                {data.availability.state !== "live" && <p className="m-0 mt-2.5 text-xs text-ink-subtext">{data.availability.reason ?? m.run_view_stale()}</p>}
                {data.availability.state !== "unavailable" && stages && <div className="mt-2.5 border-t border-border-variant pt-2.5">
                  <Phases model={model} phases={stages.phases} current={stages.current} progress={count} setupNames={stages.names} className="flex-col gap-y-2" />
                </div>}
              </>}
              {model.sentence.watcher && <p className="m-0 mt-2.5 break-words text-sm"><Sentence text={autoDir(model.sentence.watcher)} tone={model.tone} /></p>}
              {time && <p className="m-0 mt-2.5 border-t border-border-variant pt-2.5 text-xs text-ink-subtext">{footerNames ? footer.split(footerNames).map((part, index) => untimed.includes(part) ? <span key={index} className="whitespace-nowrap">{part}</span> : part) : footer}</p>}
            </Popover.Popup>
          </Popover.Positioner>
        </Popover.Portal>
      </Popover.Root>
    </EscapeScope>
  );
}
