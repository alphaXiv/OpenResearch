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
  "project-menu-label [display:inline-flex] [align-items:center] [gap:8px] [min-width:0] [overflow:hidden]",
  "[text-overflow:ellipsis] [white-space:nowrap]",
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
    <div className="rail-brand [display:flex] [align-items:center] [gap:4px] [height:64px] [padding:8px] [border-bottom:1px_solid_var(--border)] [flex-shrink:0] [&_.project-switcher]:[position:relative] [&_.project-switcher]:[flex:1] [&_.project-switcher]:[align-self:stretch] [&_.project-switcher]:[min-width:0] [&_.project-back]:[flex-shrink:0] [&_.brand]:[display:flex] [&_.brand]:[align-items:center] [&_.brand]:[justify-content:space-between] [&_.brand]:[gap:8px] [&_.brand]:[width:100%] [&_.brand]:[height:100%] [&_.brand]:[min-width:0] [&_.brand]:[font-weight:var(--fw-semibold)] [&_.brand]:[font-size:var(--fs-base)] [&_.brand]:[color:var(--text)] [&_.brand]:[padding:4px_6px] [&_.brand]:[border:1px_solid_transparent] [&_.brand]:[border-radius:var(--radius-sm)] [&_.brand:hover]:[background:var(--surface)] [&_.brand:hover]:[border-color:var(--border)] [&_.brand.open]:[background:var(--surface)] [&_.brand.open]:[border-color:var(--border)] [&_.brand_svg]:[flex-shrink:0] [&_.brand-project-copy]:[display:flex] [&_.brand-project-copy]:[flex-direction:column] [&_.brand-project-copy]:[gap:3px] [&_.brand-project-copy]:[min-width:0] [&_.brand-project-copy]:[line-height:1.15] [&_.brand-project-copy]:[text-align:left] [&_.brand-project-label]:[color:var(--muted)] [&_.brand-project-label]:[font-size:var(--fs-2xs)] [&_.brand-project-label]:[font-weight:var(--fw-medium)] [&_.brand-project-label]:[letter-spacing:0.04em] [&_.brand-project-label]:[text-transform:uppercase] [&_.brand_.brand-project]:[min-width:0] [&_.brand_.brand-project]:[overflow:hidden] [&_.brand_.brand-project]:[text-overflow:ellipsis] [&_.brand_.brand-project]:[white-space:nowrap] [&_.brand_.brand-project]:[font-size:var(--fs-2xl)] [&_.project-chevron]:[color:var(--muted)] [&_.project-chevron]:[opacity:0] [&_.project-chevron]:[transition:transform_120ms_ease] [&_.brand:hover_.project-chevron]:[opacity:1] [&_.brand.open_.project-chevron]:[opacity:1] [&_.brand.open_.project-chevron]:[transform:rotate(180deg)] [&_.project-menu]:[left:0] [&_.project-menu]:[width:210px] [&_.project-menu]:[z-index:70]">
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
          <div className="option-menu [position:absolute] [bottom:calc(100%_+_8px)] [left:0] [max-height:380px] [display:flex] [flex-direction:column] [background:var(--base)] [border:1px_solid_var(--border)] [border-radius:var(--radius-lg)] [box-shadow:0_12px_32px_rgba(0,_0,_0,_0.18)] [z-index:50] [overflow:hidden] [min-width:190px] [padding:6px] [&.align-right]:[left:auto] [&.align-right]:[right:0] [&.drop-down]:[bottom:auto] [&.drop-down]:[top:calc(100%_+_4px)] [&.session-menu]:[left:auto] [&.session-menu]:[right:6px] [&.session-menu]:[top:calc(100%_-_2px)] [&.session-menu]:[min-width:140px] drop-down project-menu">
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
