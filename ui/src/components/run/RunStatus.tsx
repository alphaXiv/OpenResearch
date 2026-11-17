import { Popover } from "@base-ui/react/popover";
import { ArrowUpRight, RefreshCw } from "lucide-react";
import { useContext, useRef, useState, type KeyboardEvent } from "react";
import type { Run } from "../../api";
import { autoDir } from "../../i18n";
import { m } from "../../paraglide/messages.js";
import { plainSentence } from "../../runMonitor/format";
import { monitorBehind, type RunModel } from "../../runMonitor/model";
import { foldedStages } from "../../runMonitor/stages";
import { capitalized, progressWords } from "../../runMonitor/words";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../../tabPreview";
import { WorkspaceDomContext } from "../../WorkspaceDom";
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
  const monitor = useRunMonitors([model.run], () => open)[0];
  const data = monitor && monitor.status !== "loading" ? monitor.data : null;
  const behind = data && monitorBehind(model.run.monitor, data);
  const stages = data && !behind ? foldedStages(data, model) : null;
  const progress = data?.progress;
  const count = data && progress?.done != null ? plainSentence(capitalized(progressWords(data.phases[progress.phase], progress.done))) : "";
  const time = [model.eta, model.etaDetail].filter(Boolean).join(" · ") || (!model.live ? model.sentence.text : "");
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
          <Popover.Positioner side="top" align="start" sideOffset={7} collisionPadding={12} className="z-50">
            <Popover.Popup initialFocus={false} finalFocus={false} onKeyDown={onKeyDown}
              className="w-80 max-w-[calc(100vw-1.5rem)] max-h-(--available-height) overflow-y-auto rounded-lg border border-border bg-background p-3.5 text-text shadow-menu outline-none"
            >
              <Popover.Title className="m-0 text-sm font-medium">
                {onOpenRunPage ? <Button variant="ghost" role="link" className="tool-target min-h-6 w-full justify-start gap-1 whitespace-normal p-0 text-start text-sm font-medium"
                  {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => { close(); onOpenRunPage(model.run, intent); })}
                >{autoDir(model.name)}<ArrowUpRight size={13} className="shrink-0" aria-hidden="true" /></Button> : autoDir(model.name)}
              </Popover.Title>
              {model.where && <Popover.Description className="m-0 mt-1 text-xs text-ink-subtext">{model.where}</Popover.Description>}
              {monitor?.status === "error" ? <div className="mt-3 text-xs text-ink-subtext" role="alert">
                {m.run_monitor_load_failed()} <Button size="small" variant="ghost" onClick={monitor.retry}><RefreshCw size={13} />{m.app_retry()}</Button>
              </div> : monitor?.status === "loading" || behind ? <p className="m-0 mt-3 text-xs text-ink-subtext" role="status">{m.common_loading()}</p> : data && stages && <>
                {data.availability.state !== "live" && <p className="m-0 mt-3 text-xs text-ink-subtext">{data.availability.reason ?? m.run_view_stale()}</p>}
                {(data.availability.state !== "unavailable" && (data.phases.length > 0 || data.setup != null || data.stageOrder.first === null)) && <div className="mt-3">
                  <Phases model={model} phases={stages.phases} current={stages.current} progress={count} setupNames={stages.names} className="flex-col gap-y-2" />
                </div>}
              </>}
              {model.sentence.watcher && <p className="m-0 mt-3 break-words text-sm"><Sentence text={autoDir(model.sentence.watcher)} tone={model.tone} /></p>}
              {time && <p className="m-0 mt-3 border-t border-border pt-3 text-xs text-ink-subtext">{model.settingUp || !model.live ? time : m.run_whole_time({ time })}</p>}
            </Popover.Popup>
          </Popover.Positioner>
        </Popover.Portal>
      </Popover.Root>
    </EscapeScope>
  );
}
