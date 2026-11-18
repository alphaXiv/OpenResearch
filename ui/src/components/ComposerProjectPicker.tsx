import { useContext } from "react";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { DirectionProvider } from "@base-ui/react/direction-provider";
import { Menu } from "@base-ui/react/menu";
import { Check, ChevronDown, FolderOpen, MessageCircleDashed, Plus } from "lucide-react";
import type { Project, ProjectActivity } from "../api";
import { m } from "../paraglide/messages.js";
import { Button, MENU_LABEL, MENU_POPUP, MENU_SEPARATOR, MenuItem } from "./ui";

export function ComposerProjectPicker({ projects, activity, projectId, projectName, onSelect, onNoProject, onNewProject }: {
  projects: Project[];
  activity: ProjectActivity[];
  projectId: string;
  projectName: string;
  onSelect: (id: string) => void;
  onNoProject: () => void;
  onNewProject: () => void;
}) {
  const { portalContainer } = useContext(WorkspaceDomContext);

  const lastActivity = new Map(activity.map((row) => [row.projectId, row.lastActivityAt]));
  const scratch = projects.find((project) => project.id === projectId)?.scratch ?? false;
  const recent = projects.filter((project) => !project.scratch).sort((a, b) => Number(b.id === projectId) - Number(a.id === projectId) || Math.max(lastActivity.get(b.id) ?? 0, b.updatedAt) - Math.max(lastActivity.get(a.id) ?? 0, a.updatedAt));
  return <div className="relative mb-2 w-fit max-w-full">
    <DirectionProvider direction={typeof document !== "undefined" && document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
      <Menu.Root modal={false}>
        <Menu.Trigger render={<Button className="max-w-full gap-2 font-normal" />}>
          {scratch ? <MessageCircleDashed size={16} className="shrink-0" /> : <FolderOpen size={16} className="shrink-0" />}<span className="truncate">{projectName}</span><ChevronDown size={12} className="shrink-0 text-subtext" />
        </Menu.Trigger>
        <Menu.Portal container={portalContainer}>
          <Menu.Positioner className="z-50" positionMethod="fixed" side="top" align="start" sideOffset={8} collisionPadding={16}>
            <Menu.Popup aria-label={m.projects_home_projects()} className={`${MENU_POPUP} max-h-(--available-height) w-64 max-w-[calc(100vw-2rem)]`}>
              <div className={MENU_LABEL}>{m.chat_recents()}</div>
              <Menu.RadioGroup value={projectId} className="max-h-64 overflow-y-auto">
                {recent.map((project) => <Menu.RadioItem nativeButton key={project.id} value={project.id} closeOnClick render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => onSelect(project.id)}>
                  <span className="truncate">{project.name}</span>{project.id === projectId && <Check size={14} className="shrink-0" />}
                </Menu.RadioItem>)}
              </Menu.RadioGroup>
              <Menu.Separator className={MENU_SEPARATOR} />
              <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => { if (!scratch) onNoProject(); }}>
                <span className="flex items-center gap-2"><MessageCircleDashed size={14} />{m.no_project()}</span>{scratch && <Check size={14} className="shrink-0" />}
              </Menu.Item>
              <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={onNewProject}><span className="flex items-center gap-2"><Plus size={14} />{m.projects_home_new_project()}</span></Menu.Item>
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
    </DirectionProvider>
  </div>;
}
