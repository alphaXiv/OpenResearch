import { DirectionProvider } from "@base-ui/react/direction-provider";
import { Menu } from "@base-ui/react/menu";
import { Check, ChevronDown, FolderOpen, Plus } from "lucide-react";
import type { Project, ProjectActivity } from "../api";
import { m } from "../paraglide/messages.js";
import { Button, MenuItem } from "./ui";

export function ComposerProjectPicker({ projects, activity, projectId, projectName, onSelect, onNewProject }: {
  projects: Project[];
  activity: ProjectActivity[];
  projectId: string;
  projectName: string;
  onSelect: (id: string) => void;
  onNewProject: () => void;
}) {
  const lastActivity = new Map(activity.map((row) => [row.projectId, row.lastActivityAt]));
  const recent = [...projects].sort((a, b) => Number(b.id === projectId) - Number(a.id === projectId) || Math.max(lastActivity.get(b.id) ?? 0, b.updatedAt) - Math.max(lastActivity.get(a.id) ?? 0, a.updatedAt));
  return <div className="relative mb-2 w-fit max-w-full">
    <DirectionProvider direction={typeof document !== "undefined" && document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
      <Menu.Root modal={false}>
        <Menu.Trigger render={<Button className="max-w-full gap-2 font-normal" />}>
          <FolderOpen size={16} className="shrink-0" /><span className="truncate">{projectName}</span><ChevronDown size={12} className="shrink-0 text-subtext" />
        </Menu.Trigger>
        <Menu.Portal>
          <Menu.Positioner className="z-50" positionMethod="fixed" side="top" align="start" sideOffset={8} collisionPadding={16}>
            <Menu.Popup aria-label={m.projects_home_projects()} className="outline-none max-h-(--available-height) overflow-y-auto w-64 max-w-[calc(100vw-2rem)] rounded-lg border border-border bg-background p-1.5 shadow-menu">
              <div className="px-2 py-1 text-sm text-subtext">{m.chat_recents()}</div>
              <Menu.RadioGroup value={projectId} className="max-h-64 overflow-y-auto pb-1.5">
                {recent.map((project) => <Menu.RadioItem nativeButton key={project.id} value={project.id} closeOnClick render={<MenuItem size="compact" className="text-sm data-highlighted:bg-surface" />} onClick={() => onSelect(project.id)}>
                  <span className="truncate">{project.name}</span>{project.id === projectId && <Check size={14} className="shrink-0" />}
                </Menu.RadioItem>)}
              </Menu.RadioGroup>
              <Menu.Separator className="mb-1 border-t border-border" />
              <Menu.Item nativeButton render={<MenuItem size="compact" className="text-sm data-highlighted:bg-surface" />} onClick={onNewProject}><span className="flex items-center gap-2"><Plus size={14} />{m.projects_home_new_project()}</span></Menu.Item>
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
    </DirectionProvider>
  </div>;
}
