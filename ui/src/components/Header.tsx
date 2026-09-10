import { m } from "../paraglide/messages.js";
import {
  ArrowLeft,
  ChevronDown,
  FolderGit2,
  FolderPlus,
  History,
  PanelLeft,
} from "lucide-react";
import { useEffect, useRef } from "react";
import { usePopover } from "./ModelPicker";
import { IconButton, MenuItem } from "./ui";

const PROJECT_MENU_LABEL_CLASS_NAME = [
  "project-menu-label inline-flex items-center gap-2 min-w-0 overflow-hidden",
  "text-ellipsis whitespace-nowrap",
].join(" ");

/** Top row of the agents rail: back to the projects page + the current
 *  project's name. Settings sections live in the rail nav below. */
export function RailHeader({
  projectName,
  onHome,
  onNewProject,
  onRepository,
  onCollapse,
}: {
  projectName: string;
  onHome: () => void;
  onNewProject: () => void;
  onRepository: () => void;
  /** Hide the rail (a matching reopen button lives in the chat header). */
  onCollapse?: () => void;
}) {
  const { open, setOpen, ref } = usePopover();
  const projectButtonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    const restoreFocus = (event: KeyboardEvent) => {
      if (event.key === "Escape") projectButtonRef.current?.focus();
    };
    document.addEventListener("keydown", restoreFocus, true);
    return () => document.removeEventListener("keydown", restoreFocus, true);
  }, [open]);

  return (
    <div className="rail-brand px-3 py-1.5 border-b border-border shrink-0">
      <div className="project-switcher relative min-w-0" ref={ref}>
        <div className="flex h-7 items-center justify-between gap-1 px-0.5">
          <IconButton size="small" className="project-back text-text" aria-label={m.header_all_projects()} onClick={onHome}>
            <ArrowLeft size={18} />
          </IconButton>
          <span className="brand-project-label flex-1 text-xs font-medium tracking-wide uppercase text-subtext">{m.header_project()}</span>
          {onCollapse && (
            <IconButton
              size="small"
              data-tip={m.header_hide_sidebar()}
              data-tip-align="end"
              aria-label={m.header_hide_sidebar()}
              onClick={onCollapse}
            >
              <PanelLeft size={18} />
            </IconButton>
          )}
        </div>
        <button
          ref={projectButtonRef}
          className={`brand group flex h-8 w-full min-w-0 items-center gap-2 rounded-md px-2 text-start text-text hover:bg-surface focus-visible:outline-2 focus-visible:outline-text ${open ? "open bg-surface" : ""}`}
          onClick={() => setOpen((value) => !value)}
          aria-expanded={open}
        >
          <span className="brand-project min-w-0 flex-1 truncate text-xl font-semibold">{projectName}</span>
          <ChevronDown className={`project-chevron shrink-0 text-muted transition-opacity group-hover:opacity-100 group-focus-visible:opacity-100 ${open ? "rotate-180 opacity-100" : "opacity-0"}`} size={14} />
        </button>
        {open && (
          <div className="option-menu absolute bottom-[calc(100%_+_8px)] start-0 max-h-95 flex flex-col bg-background border border-border rounded-lg shadow-menu overflow-hidden min-w-47.5 p-1.5 [&.align-right]:start-auto [&.align-right]:end-0 [&.drop-down]:bottom-auto [&.drop-down]:top-[calc(100%_+_4px)] [&.session-menu]:start-auto [&.session-menu]:end-1.5 [&.session-menu]:top-[calc(100%_-_2px)] [&.session-menu]:min-w-35 drop-down project-menu w-52.5 z-70">
            <MenuItem
              onClick={() => {
                setOpen(false);
                onRepository();
              }}
            >
              <span className={PROJECT_MENU_LABEL_CLASS_NAME}>
                <FolderGit2 size={14} />{m.header_configure_repository()}
              </span>
            </MenuItem>
            <MenuItem
              onClick={() => {
                setOpen(false);
                onHome();
              }}
            >
              <span className={PROJECT_MENU_LABEL_CLASS_NAME}><History size={14} />{m.header_all_projects()}</span>
            </MenuItem>
            <MenuItem
              onClick={() => {
                projectButtonRef.current?.focus();
                setOpen(false);
                onNewProject();
              }}
            >
              <span className={PROJECT_MENU_LABEL_CLASS_NAME}><FolderPlus size={14} />{m.header_create_a_new_project()}</span>
            </MenuItem>
          </div>
        )}
      </div>

    </div>
  );
}
