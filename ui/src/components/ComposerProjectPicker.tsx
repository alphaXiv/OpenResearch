import { useContext, useRef, useState } from "react";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { DirectionProvider } from "@base-ui/react/direction-provider";
import { Popover } from "@base-ui/react/popover";
import { Check, ChevronDown, Import, MessageCircleDashed, Search, Settings } from "lucide-react";
import type { Project, ProjectActivity, ProjectIcon } from "../api";
import { m } from "../paraglide/messages.js";
import { mutUpdateProject } from "../queries/projects";
import { useWorkspace } from "../workspace";
import { ProjectIconButton, ProjectIconTile } from "./ProjectIcon";
import { Button, IconButton, MENU_LABEL, MenuItem, Tooltip, showAlert } from "./ui";

/** Where the next chat goes. A new project offers a name and an icon, and Import; all optional. */
export function ComposerProjectPicker({ projects, activity, projectId, projectName, draftProject, onSelect, onNewProject, onImport, onOpenSettings }: {
  projects: Project[];
  activity: ProjectActivity[];
  projectId: string;
  projectName: string;
  /** The new project the next chat starts. */
  draftProject?: Project;
  onSelect: (id: string) => void;
  onNewProject: () => void;
  onImport: () => void;
  onOpenSettings: (project: Project) => void;
}) {
  const { portalContainer } = useContext(WorkspaceDomContext);
  const searchRef = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState("");

  const lastActivity = new Map(activity.map((row) => [row.projectId, row.lastActivityAt]));
  const current = projects.find((project) => project.id === projectId);
  const unnamed = current?.unnamed ?? false;
  const recent = projects.filter((project) => !project.unnamed).sort((a, b) => Number(b.id === projectId) - Number(a.id === projectId) || Math.max(lastActivity.get(b.id) ?? 0, b.updatedAt) - Math.max(lastActivity.get(a.id) ?? 0, a.updatedAt));
  const query = filter.trim().toLowerCase();
  const matches = query ? recent.filter((project) => project.name.toLowerCase().includes(query)) : recent;

  const choose = (action: () => void) => {
    setOpen(false);
    action();
  };
  const moveFocus = (event: React.KeyboardEvent<HTMLElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const stops = Array.from(event.currentTarget.querySelectorAll<HTMLElement>("input, [data-nav]"));
    const step = event.key === "ArrowDown" ? 1 : -1;
    stops[(stops.indexOf(document.activeElement as HTMLElement) + step + stops.length) % stops.length]?.focus();
  };

  return <div className="relative mb-2 flex w-fit max-w-full flex-wrap items-center gap-1.5">
    <DirectionProvider direction={typeof document !== "undefined" && document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
      <Popover.Root open={open} onOpenChange={(next) => { setOpen(next); setFilter(""); }}>
        <Popover.Trigger render={<Button className="max-w-full gap-2 font-normal" />}>
          {draftProject ? <span className="truncate">{m.projects_home_new_project()}</span> : <>
            {current && <ProjectIconTile project={current} size="xs" />}<span className="truncate">{projectName}</span>
          </>}
          <ChevronDown size={12} className="shrink-0 text-subtext" />
        </Popover.Trigger>
        <Popover.Portal container={portalContainer}>
          <Popover.Positioner className="z-50" positionMethod="fixed" side="top" align="start" sideOffset={8} collisionPadding={16}>
            <Popover.Popup initialFocus={searchRef} onKeyDown={moveFocus} aria-label={m.projects_home_projects()}
              className="flex max-h-(--available-height) w-80 max-w-[calc(100vw-2rem)] flex-col overflow-hidden rounded-lg border border-border bg-background shadow-dropdown outline-none">
              <div className="flex shrink-0 items-center gap-2 border-b border-border-variant px-3">
                <Search size={14} aria-hidden="true" className="shrink-0 text-muted" />
                <input
                  ref={searchRef}
                  type="text"
                  value={filter}
                  aria-label={m.project_picker_search()}
                  placeholder={m.project_picker_search()}
                  autoComplete="off"
                  spellCheck={false}
                  className="min-w-0 flex-1 border-0 bg-transparent px-0 py-2.5 text-sm outline-none placeholder:text-muted"
                  onChange={(event) => setFilter(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key !== "Enter" || event.nativeEvent.isComposing || event.keyCode === 229 || !matches[0]) return;
                    event.preventDefault();
                    choose(() => onSelect(matches[0].id));
                  }}
                />
              </div>
              <div className="min-h-0 flex-1 overflow-y-auto p-1">
                {!query && <div className={MENU_LABEL}>{m.chat_recents()}</div>}
                {matches.map((project) => (
                  <div key={project.id} className="group/row relative">
                    <MenuItem data-nav size="compact" className="min-h-8 justify-start gap-2.5 pe-9 text-sm" aria-current={project.id === projectId || undefined} onClick={() => choose(() => onSelect(project.id))}>
                      <ProjectIconTile project={project} size="xs" />
                      <span className="min-w-0 flex-1 truncate">{project.name}</span>
                      {project.id === projectId && <Check size={14} aria-hidden="true" className="shrink-0 text-subtext" />}
                    </MenuItem>
                    <Tooltip interactive content={m.sidebar_project_settings()} className="absolute end-1 top-1/2 -translate-y-1/2 rounded-sm opacity-0 group-hover/row:opacity-100 group-focus-within/row:opacity-100">
                      <IconButton size="small" className="text-subtext" aria-label={`${m.sidebar_project_settings()} · ${project.name}`} onClick={() => choose(() => onOpenSettings(project))}>
                        <Settings size={14} />
                      </IconButton>
                    </Tooltip>
                  </div>
                ))}
                {matches.length === 0 && <div role="status" className="px-2 py-3 text-sm text-muted">{m.project_picker_no_matches()}</div>}
              </div>
              <div className="shrink-0 border-t border-border-variant p-1">
                <MenuItem data-nav className="justify-start gap-2.5" onClick={() => choose(() => unnamed ? onSelect(projectId) : onNewProject())}>
                  <span className="flex size-5 shrink-0 items-center justify-center text-subtext"><MessageCircleDashed size={16} aria-hidden="true" /></span>
                  <span className="min-w-0 flex-1 truncate">{m.projects_home_new_project()}</span>
                  {unnamed && <Check size={14} aria-hidden="true" className="shrink-0 text-subtext" />}
                </MenuItem>
              </div>
            </Popover.Popup>
          </Popover.Positioner>
        </Popover.Portal>
      </Popover.Root>
    </DirectionProvider>
    {draftProject && <>
      <ProjectIdentity key={draftProject.id} project={draftProject} />
      <Tooltip content={m.new_project_import_hint()}>
        <Button variant="ghost" className="gap-1.5 font-normal text-subtext" onClick={onImport}><Import size={14} />{m.new_project_import()}</Button>
      </Tooltip>
    </>}
  </div>;
}

/** The icon and name of a project that has neither yet; both save as they are set, and the name is optional. */
function ProjectIdentity({ project }: { project: Project }) {
  const workspace = useWorkspace();
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
    <div className="flex h-8 min-w-0 items-center gap-1.5 rounded-md border border-border bg-background ps-1.5 pe-2.5 focus-within:border-muted">
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
    </div>
  );
}
