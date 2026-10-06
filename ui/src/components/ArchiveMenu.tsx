import { useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Ellipsis } from "lucide-react";
import { m } from "../paraglide/messages.js";
import type { Experiment } from "../api";
import { usePopover } from "./ModelPicker";
import { IconButton, MenuItem } from "./ui";

export type ArchiveActions = {
  archiveOnly: boolean;
  archiveAbove: boolean;
  archiveDown: boolean;
  restoreOnly: boolean;
  restoreAbove: boolean;
  restoreDown: boolean;
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
    });
  }
  return actions;
}

export function ArchiveMenu({ id, name, actions, onArchive, compact = false }: {
  id: string;
  name: string;
  actions: ArchiveActions;
  onArchive: (id: string, direction: "ancestors" | "descendants" | "only", archived: boolean) => void;
  compact?: boolean;
}) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menu = usePopover(triggerRef);
  const [position, setPosition] = useState({ top: 0, left: 0 });
  const toggle = () => {
    if (!menu.open && triggerRef.current) {
      const rect = triggerRef.current.getBoundingClientRect();
      const menuHeight = Object.values(actions).filter(Boolean).length * 32 + 12;
      const below = window.innerHeight - rect.bottom >= menuHeight;
      setPosition({
        top: below ? rect.bottom + 4 : Math.max(4, rect.top - menuHeight),
        left: Math.max(4, Math.min(rect.right - 176, window.innerWidth - 180)),
      });
    }
    menu.setOpen((open) => !open);
  };
  const choose = (direction: "ancestors" | "descendants" | "only", archived: boolean) => {
    menu.setOpen(false);
    onArchive(id, direction, archived);
  };

  return (
    <div className="nodrag ms-auto shrink-0">
      {compact ? (
        <IconButton
          type="button"
          ref={triggerRef}
          size="small"
          className="node-action"
          aria-label={m.tree_archive_manage_experiment({ name })}
          aria-expanded={menu.open}
          onClick={toggle}
        >
          <Ellipsis size={15} aria-hidden="true" />
        </IconButton>
      ) : (
        <button
          type="button"
          className="btn inline-flex h-7 items-center justify-center rounded-sm border border-border bg-background px-2.5 text-sm text-text hover:bg-surface"
          ref={triggerRef}
          aria-label={m.tree_archive_manage_experiment({ name })}
          aria-expanded={menu.open}
          onClick={toggle}
        >
          {m.tree_archive_manage()}
        </button>
      )}
      {menu.open && createPortal(
        <div ref={menu.ref} className="option-menu fixed z-70 min-w-44 rounded-lg border border-border bg-background p-1.5 shadow-menu" style={position}>
          {actions.archiveAbove && <MenuItem onClick={() => choose("ancestors", true)}>{m.tree_archive_above()}</MenuItem>}
          {actions.archiveOnly && <MenuItem onClick={() => choose("only", true)}>{m.tree_archive_only()}</MenuItem>}
          {actions.archiveDown && <MenuItem onClick={() => choose("descendants", true)}>{m.tree_archive_down()}</MenuItem>}
          {actions.restoreOnly && <MenuItem onClick={() => choose("only", false)}>{m.tree_restore_only()}</MenuItem>}
          {actions.restoreAbove && <MenuItem onClick={() => choose("ancestors", false)}>{m.tree_restore_above()}</MenuItem>}
          {actions.restoreDown && <MenuItem onClick={() => choose("descendants", false)}>{m.tree_restore_down()}</MenuItem>}
        </div>, document.body,
      )}
    </div>
  );
}
