import { useId, useState } from "react";
import { DirectionProvider } from "@base-ui/react/direction-provider";
import { Menu } from "@base-ui/react/menu";
import { Ellipsis } from "lucide-react";
import { m } from "../paraglide/messages.js";
import type { Experiment } from "../api";
import { Button, IconButton, MenuItem } from "./ui";

export type ArchiveActions = {
  archiveOnly: boolean;
  archiveAbove: boolean;
  archiveDown: boolean;
  restoreOnly: boolean;
  restoreAbove: boolean;
  restoreDown: boolean;
  remove: boolean;
};

export function archiveActionsByExperiment(experiments: Experiment[]): Map<string, ArchiveActions> {
  const children = new Map<string, Experiment[]>();
  const byId = new Map(experiments.map((experiment) => [experiment.id, experiment]));
  for (const experiment of experiments) {
    if (!experiment.parentExperimentId) continue;
    const siblings = children.get(experiment.parentExperimentId) ?? [];
    siblings.push(experiment);
    children.set(experiment.parentExperimentId, siblings);
  }
  const archivedBelow = new Map<string, boolean>();
  const activeBelow = new Map<string, boolean>();
  function hasArchivedBelow(id: string): boolean {
    const cached = archivedBelow.get(id);
    if (cached !== undefined) return cached;
    const found = (children.get(id) ?? []).some((child) => child.archived || hasArchivedBelow(child.id));
    archivedBelow.set(id, found);
    return found;
  }
  function hasActiveBelow(id: string): boolean {
    const cached = activeBelow.get(id);
    if (cached !== undefined) return cached;
    const found = (children.get(id) ?? []).some((child) => !child.archived || hasActiveBelow(child.id));
    activeBelow.set(id, found);
    return found;
  }
  const actions = new Map<string, ArchiveActions>();
  for (const experiment of experiments) {
    let activeAbove = false;
    let archivedAbove = false;
    let parent = experiment.parentExperimentId ? byId.get(experiment.parentExperimentId) : undefined;
    while (parent) {
      if (parent.archived) archivedAbove = true;
      else activeAbove = true;
      parent = parent.parentExperimentId ? byId.get(parent.parentExperimentId) : undefined;
    }
    actions.set(experiment.id, {
      archiveOnly: !experiment.archived,
      archiveAbove: !experiment.archived && activeAbove,
      archiveDown: !experiment.archived && hasActiveBelow(experiment.id),
      restoreOnly: experiment.archived,
      restoreAbove: !experiment.archived && archivedAbove,
      restoreDown: hasArchivedBelow(experiment.id),
      remove: !children.has(experiment.id),
    });
  }
  return actions;
}

export function ArchiveMenu({ id, name, actions, onArchive, onDelete, compact = false }: {
  id: string;
  name: string;
  actions: ArchiveActions;
  onArchive: (id: string, direction: "ancestors" | "descendants" | "only", archived: boolean) => void;
  onDelete: (id: string) => void;
  compact?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const triggerId = useId();

  return (
    <div className="nodrag ms-auto shrink-0">
      <DirectionProvider direction={typeof document !== "undefined" && document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
        <Menu.Root open={open} onOpenChange={setOpen} triggerId={triggerId} modal={false}>
          <Menu.Trigger
            id={triggerId}
            render={compact
              ? <IconButton type="button" size="small" className="node-action" aria-label={m.tree_archive_manage_experiment({ name })} />
              : <Button type="button" size="small" aria-label={m.tree_archive_manage_experiment({ name })} />
            }
            // XYFlow consumes mousedown before React's bubble handlers receive it.
            onMouseDown={(event) => event.preventBaseUIHandler()}
            onClick={(event) => { event.preventBaseUIHandler(); setOpen((value) => !value); }}
          >
            {compact ? <Ellipsis size={16} aria-hidden="true" /> : m.tree_archive_manage()}
          </Menu.Trigger>
          <Menu.Portal>
            <Menu.Positioner className="z-70" positionMethod="fixed" align="end" sideOffset={4} collisionPadding={4}>
              <Menu.Popup className="outline-none max-h-(--available-height) overflow-y-auto option-menu min-w-44 rounded-lg border border-border bg-background p-1.5 shadow-menu">
                {actions.archiveAbove && <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => onArchive(id, "ancestors", true)}>{m.tree_archive_above()}</Menu.Item>}
                {actions.archiveOnly && <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => onArchive(id, "only", true)}>{m.tree_archive_only()}</Menu.Item>}
                {actions.archiveDown && <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => onArchive(id, "descendants", true)}>{m.tree_archive_down()}</Menu.Item>}
                {actions.restoreOnly && <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => onArchive(id, "only", false)}>{m.tree_restore_only()}</Menu.Item>}
                {actions.restoreAbove && <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => onArchive(id, "ancestors", false)}>{m.tree_restore_above()}</Menu.Item>}
                {actions.restoreDown && <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => onArchive(id, "descendants", false)}>{m.tree_restore_down()}</Menu.Item>}
                {actions.remove && <Menu.Item nativeButton render={<MenuItem danger className="data-highlighted:bg-surface" />} onClick={() => { if (window.confirm(m.tree_delete_experiment_confirm({ name }))) onDelete(id); }}>{m.tree_delete_experiment()}</Menu.Item>}
              </Menu.Popup>
            </Menu.Positioner>
          </Menu.Portal>
        </Menu.Root>
      </DirectionProvider>
    </div>
  );
}
