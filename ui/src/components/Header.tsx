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
import { ICON_BUTTON_CLASS_NAME, MODEL_ITEM_CLASS_NAME } from "../styleClasses";

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
    <div className="rail-brand flex items-center gap-1 h-16 p-2 border-b border-b-border shrink-0 [&_.project-switcher]:relative [&_.project-switcher]:flex-1 [&_.project-switcher]:self-stretch [&_.project-switcher]:min-w-0 [&_.project-back]:shrink-0 [&_.brand]:flex [&_.brand]:items-center [&_.brand]:justify-between [&_.brand]:gap-2 [&_.brand]:w-full [&_.brand]:h-full [&_.brand]:min-w-0 [&_.brand]:font-semibold [&_.brand]:text-base [&_.brand]:text-text [&_.brand]:py-1 [&_.brand]:px-1.5 [&_.brand]:border [&_.brand]:border-transparent [&_.brand]:rounded-sm [&_.brand:hover]:bg-surface [&_.brand:hover]:border-border [&_.brand.open]:bg-surface [&_.brand.open]:border-border [&_.brand_svg]:shrink-0 [&_.brand-project-copy]:flex [&_.brand-project-copy]:flex-col [&_.brand-project-copy]:gap-[3px] [&_.brand-project-copy]:min-w-0 [&_.brand-project-copy]:leading-[1.15] [&_.brand-project-copy]:text-left [&_.brand-project-label]:text-muted [&_.brand-project-label]:text-2xs [&_.brand-project-label]:font-medium [&_.brand-project-label]:tracking-[0.04em] [&_.brand-project-label]:uppercase [&_.brand_.brand-project]:min-w-0 [&_.brand_.brand-project]:overflow-hidden [&_.brand_.brand-project]:text-ellipsis [&_.brand_.brand-project]:whitespace-nowrap [&_.brand_.brand-project]:text-2xl [&_.project-chevron]:text-muted [&_.project-chevron]:opacity-0 [&_.project-chevron]:transition-transform [&_.project-chevron]:duration-120 [&_.project-chevron]:ease-standard [&_.brand:hover_.project-chevron]:opacity-100 [&_.brand.open_.project-chevron]:opacity-100 [&_.brand.open_.project-chevron]:rotate-180 [&_.project-menu]:left-0 [&_.project-menu]:w-52.5 [&_.project-menu]:z-70">
      <button
        className={`${ICON_BUTTON_CLASS_NAME} project-back`}
        data-tip="All projects"
        data-tip-align="start"
        aria-label="All projects"
        onClick={onHome}
      >
        <ArrowLeft size={15} />
      </button>
      <div className="project-switcher" ref={ref}>
        <button
          ref={projectButtonRef}
          className={`brand${open ? " open" : ""}`}
          onClick={() => setOpen((value) => !value)}
          aria-expanded={open}
        >
          <span className="brand-project-copy">
            <span className="brand-project-label">Project</span>
            <span className="brand-project">{projectName}</span>
          </span>
          <ChevronDown className="project-chevron" size={14} />
        </button>
        {open && (
          <div className="option-menu absolute bottom-[calc(100%_+_8px)] left-0 max-h-95 flex flex-col bg-background border border-border rounded-lg shadow-[0_12px_32px_rgba(0,_0,_0,_0.18)] z-50 overflow-hidden min-w-47.5 p-1.5 [&.align-right]:left-auto [&.align-right]:right-0 [&.drop-down]:bottom-auto [&.drop-down]:top-[calc(100%_+_4px)] [&.session-menu]:left-auto [&.session-menu]:right-1.5 [&.session-menu]:top-[calc(100%_-_2px)] [&.session-menu]:min-w-35 drop-down project-menu">
            <button
              className={MODEL_ITEM_CLASS_NAME}
              onClick={() => {
                setOpen(false);
                onRepository();
              }}
            >
              <span className={PROJECT_MENU_LABEL_CLASS_NAME}>
                <FolderGit2 size={14} />Configure Repository
              </span>
            </button>
            <button
              className={MODEL_ITEM_CLASS_NAME}
              onClick={() => {
                setOpen(false);
                onHome();
              }}
            >
              <span className={PROJECT_MENU_LABEL_CLASS_NAME}><History size={14} />All projects</span>
            </button>
            <button
              className={MODEL_ITEM_CLASS_NAME}
              onClick={() => {
                projectButtonRef.current?.focus();
                setOpen(false);
                onNewProject();
              }}
            >
              <span className={PROJECT_MENU_LABEL_CLASS_NAME}><FolderPlus size={14} />Create a new project</span>
            </button>
          </div>
        )}
      </div>
      {onCollapse && (
        <button
          className={ICON_BUTTON_CLASS_NAME}
          data-tip="Hide sidebar"
          data-tip-align="end"
          aria-label="Hide sidebar"
          onClick={onCollapse}
        >
          <PanelLeft size={15} />
        </button>
      )}
    </div>
  );
}
