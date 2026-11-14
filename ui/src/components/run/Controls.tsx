import { useMutate } from "@clo/react-mutation";
import { ChevronDown, ChevronUp, CircleStop, X } from "lucide-react";
import { useEffect, useRef } from "react";
import { m } from "../../paraglide/messages.js";
import { mutCancelRun, mutDismissShownRun } from "../../queries/projects";
import type { RunModel } from "../../runMonitor/model";
import { useWorkspace } from "../../workspace";
import { IconButton, Tooltip } from "../ui";

/** Stop while it runs; once it has ended there is nothing to stop, so the same place dismisses it. */
export function Controls({ model, expanded, onToggle }: { model: RunModel; expanded: boolean; onToggle: () => void }) {
  const workspace = useWorkspace();
  const cancel = useMutate(mutCancelRun);
  const dismiss = useMutate(mutDismissShownRun);
  const toggle = expanded ? m.run_show_less() : m.run_show_more();
  // A second press before the first has rendered must not send a second cancel.
  const stopping = useRef(false);
  useEffect(() => {
    if (!cancel.isPending && !model.run.cancelling) stopping.current = false;
  }, [cancel.isPending, model.run.cancelling]);
  function stop() {
    if (stopping.current) return;
    stopping.current = true;
    cancel.run(workspace, model.run);
  }
  return (
    <div className="flex shrink-0 items-center">
      {model.canStop && (
        <Tooltip interactive content={m.run_stop()}>
          <IconButton size="small" aria-label={m.run_stop()} disabled={cancel.isPending} onClick={stop}>
            <CircleStop size={14} />
          </IconButton>
        </Tooltip>
      )}
      {model.canDismiss && (
        <Tooltip interactive content={m.run_dismiss()}>
          <IconButton size="small" aria-label={m.run_dismiss()} onClick={() => dismiss.run(workspace, model.sessionId, model.run.id)}>
            <X size={14} />
          </IconButton>
        </Tooltip>
      )}
      <Tooltip interactive content={toggle}>
        <IconButton size="small" aria-label={toggle} aria-expanded={expanded} data-run-toggle={model.run.id} onClick={onToggle}>
          {expanded ? <ChevronDown size={14} /> : <ChevronUp size={14} />}
        </IconButton>
      </Tooltip>
    </div>
  );
}
