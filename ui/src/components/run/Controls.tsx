import { useMutate } from "@clo/react-mutation";
import { ChevronDown, ChevronUp, CircleStop, X } from "lucide-react";
import { m } from "../../paraglide/messages.js";
import { mutCancelRun, mutDismissShownRun } from "../../queries/projects";
import type { RunModel } from "../../runMonitor/model";
import { useWorkspace } from "../../workspace";
import { IconButton, Tooltip } from "../ui";
import { confirmStop } from "./confirmStop";

/** Stop while it runs; once it has ended there is nothing to stop, so the same place dismisses it. */
export function Controls({ model, expanded, onToggle, expandable = true }: { model: RunModel; expandable?: boolean; expanded: boolean; onToggle: () => void }) {
  const workspace = useWorkspace();
  const dismiss = useMutate(mutDismissShownRun);
  const toggle = expanded ? m.run_show_less() : m.run_show_more();
  const stop = () => confirmStop(workspace.dialogs, model.name, () => mutCancelRun.runAsHeadlessPromise(workspace, model.run, {}));
  return (
    <div className="flex shrink-0 items-center">
      {model.canStop && (
        <Tooltip interactive content={m.run_stop()}>
          <IconButton size="small" aria-haspopup="dialog" aria-label={m.run_stop()} onClick={stop}>
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
      {expandable && <Tooltip interactive content={toggle}>
        <IconButton size="small" aria-label={toggle} aria-expanded={expanded} data-run-toggle={model.run.id} onClick={onToggle}>
          {expanded ? <ChevronDown size={14} /> : <ChevronUp size={14} />}
        </IconButton>
      </Tooltip>}
    </div>
  );
}
