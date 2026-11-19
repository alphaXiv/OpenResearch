import { useContext, useRef, useState, type ReactNode, type RefObject } from "react";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { DirectionProvider } from "@base-ui/react/direction-provider";
import { Menu } from "@base-ui/react/menu";
import { Check, ChevronDown, FolderOpen, Import, MessageCircleDashed } from "lucide-react";
import type { Project, ProjectActivity, ProjectIcon } from "../api";
import { m } from "../paraglide/messages.js";
import { mutUpdateProject } from "../queries/projects";
import { useWorkspace } from "../workspace";
import { ProjectIconButton } from "./ProjectIcon";
import { Button, IconButton, MENU_LABEL, MENU_POPUP, MENU_SEPARATOR, MenuItem, Tooltip, showAlert } from "./ui";

/** Where the next chat goes. A project with no chat yet and no folder offers a name and an icon, and Import; all optional. */
export function ComposerProjectPicker({ projects, activity, projectId, projectName, draftProject, onSelect, onNewProject, onImport }: {
  projects: Project[];
  activity: ProjectActivity[];
  projectId: string;
  projectName: string;
  /** The project the next chat starts, while it has no chat yet and no folder. */
  draftProject?: Project;
  onSelect: (id: string) => void;
  onNewProject: () => void;
  onImport: () => void;
}) {
  const { portalContainer } = useContext(WorkspaceDomContext);

  const lastActivity = new Map(activity.map((row) => [row.projectId, row.lastActivityAt]));
  const unnamed = projects.find((project) => project.id === projectId)?.unnamed ?? false;
  const recent = projects.filter((project) => !project.unnamed).sort((a, b) => Number(b.id === projectId) - Number(a.id === projectId) || Math.max(lastActivity.get(b.id) ?? 0, b.updatedAt) - Math.max(lastActivity.get(a.id) ?? 0, a.updatedAt));

  const trigger = draftProject
    ? <Menu.Trigger render={<IconButton size="small" aria-label={m.projects_home_projects()} />}><ChevronDown size={12} /></Menu.Trigger>
    : <Menu.Trigger render={<Button className="max-w-full gap-2 font-normal" />}>
      {unnamed ? <MessageCircleDashed size={16} className="shrink-0" /> : <FolderOpen size={16} className="shrink-0" />}<span className="truncate">{projectName}</span><ChevronDown size={12} className="shrink-0 text-subtext" />
    </Menu.Trigger>;
  const menu = (anchor?: RefObject<HTMLDivElement | null>) => (
    <DirectionProvider direction={typeof document !== "undefined" && document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
      <Menu.Root modal={false}>
        {trigger}
        <Menu.Portal container={portalContainer}>
          <Menu.Positioner className="z-50" positionMethod="fixed" side="top" align="start" sideOffset={8} collisionPadding={16} anchor={anchor}>
            <Menu.Popup aria-label={m.projects_home_projects()} className={`${MENU_POPUP} max-h-(--available-height) w-64 max-w-[calc(100vw-2rem)]`}>
              <div className={MENU_LABEL}>{m.chat_recents()}</div>
              <Menu.RadioGroup value={projectId} className="max-h-64 overflow-y-auto">
                {recent.map((project) => <Menu.RadioItem nativeButton key={project.id} value={project.id} closeOnClick render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => onSelect(project.id)}>
                  <span className="truncate">{project.name}</span>{project.id === projectId && <Check size={14} className="shrink-0" />}
                </Menu.RadioItem>)}
              </Menu.RadioGroup>
              <Menu.Separator className={MENU_SEPARATOR} />
              <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => { if (!unnamed) onNewProject(); }}>
                <span className="flex items-center gap-2"><MessageCircleDashed size={14} />{m.projects_home_new_project()}</span>{unnamed && <Check size={14} className="shrink-0" />}
              </Menu.Item>
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
    </DirectionProvider>
  );

  return <div className="relative mb-2 flex w-fit max-w-full items-center gap-1">
    {draftProject ? <ProjectIdentity key={draftProject.id} project={draftProject} menu={menu} /> : menu()}
    {draftProject && <Tooltip content={m.new_project_import_hint()}>
      <Button variant="ghost" className="gap-1.5 font-normal text-subtext" onClick={onImport}><Import size={14} />{m.new_project_import()}</Button>
    </Tooltip>}
  </div>;
}

/** The icon and name of a project that has neither yet; both save as they are set, and the name is optional. */
function ProjectIdentity({ project, menu }: { project: Project; menu: (anchor: RefObject<HTMLDivElement | null>) => ReactNode }) {
  const workspace = useWorkspace();
  const chip = useRef<HTMLDivElement>(null);
  const saved = project.unnamed ? "" : project.name;
  const [name, setName] = useState(saved);

  const update = (changes: { name?: string; icon?: ProjectIcon }) =>
    mutUpdateProject.runAsHeadlessPromise(workspace, project.id, changes, {}).catch((error: unknown) => {
      setName(saved);
      showAlert(error instanceof Error ? error.message : String(error), "error");
    });
  const commitName = () => {
    const next = name.trim();
    if (next === saved) return;
    if (next) void update({ name: next });
    else setName(saved);
  };

  return (
    <div ref={chip} className="flex h-8 min-w-0 items-center gap-1.5 rounded-md border border-border bg-background ps-1.5 pe-1 focus-within:border-muted">
      <ProjectIconButton compact project={project} name={name} value={project.icon} onChange={(icon) => void update({ icon })} />
      <input
        value={name}
        aria-label={m.new_project_name_placeholder()}
        placeholder={m.new_project_name_placeholder()}
        autoComplete="off"
        spellCheck={false}
        className="h-full w-52 min-w-0 rounded-none border-0 bg-transparent p-0 text-sm text-text shadow-none outline-none placeholder:text-muted"
        onChange={(event) => setName(event.target.value)}
        onBlur={commitName}
        onKeyDown={(event) => { if (event.key === "Enter") event.currentTarget.blur(); }}
      />
      {menu(chip)}
    </div>
  );
}
