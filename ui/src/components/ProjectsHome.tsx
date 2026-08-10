import { Plus, Trash2 } from "lucide-react";
import { Wordmark } from "./Wordmark";
import { GitHubMark } from "./BackendLogos";
import { useEffect, useRef, useState } from "react";
import { deleteProject, timeAgo, type Project } from "../api";
import { NewProjectForm } from "./NewProjectForm";
import { MONO_CLASS_NAME, SMALL_BUTTON_CLASS_NAME } from "../styleClasses";

export function NewProjectDialog({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: (project: Project, githubPublicationError: string | null) => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const focusable = () =>
      [...dialog.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])',
      )];
    (focusable()[0] ?? dialog).focus();

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onCloseRef.current();
        return;
      }
      if (
        event.key === "Enter" &&
        (event.metaKey || event.ctrlKey) &&
        !event.altKey &&
        event.shiftKey
      ) {
        event.preventDefault();
        event.stopPropagation();
        return;
      }
      if (event.key !== "Tab") return;
      const controls = focusable();
      if (controls.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }
      const first = controls[0];
      const last = controls[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", handleKeyDown, true);
    return () => {
      document.removeEventListener("keydown", handleKeyDown, true);
      previousFocus?.focus();
    };
  }, []);

  return (
    <div
      className="modal-backdrop [position:fixed] [inset:0] [background:rgba(29,_27,_26,_0.4)] [display:flex] [align-items:flex-start] [justify-content:center] [padding:var(--modal-top)_16px_24px] [overflow-y:auto] [z-index:100]"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="modal [width:480px] [max-width:94vw] [max-height:calc(100vh_-_var(--modal-top)_-_48px)] [overflow-y:auto] [background:var(--base)] [border:1px_solid_var(--border)] [border-radius:var(--radius-xl)] [box-shadow:0_24px_60px_rgba(0,_0,_0,_0.22)] [padding:24px] [&_h2]:[margin:0_0_14px] [&_h2]:[font-size:var(--fs-xl)]"
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-project-dialog-title"
        tabIndex={-1}
      >
        <h2 id="new-project-dialog-title">New project</h2>
        <NewProjectForm onCancel={onClose} onCreated={onCreated} />
      </div>
    </div>
  );
}

export function ProjectsHome({
  projects,
  onOpen,
  onCreated,
  onDeleted,
}: {
  projects: Project[];
  onOpen: (id: string) => void;
  onCreated: (project: Project, githubPublicationError: string | null) => void;
  onDeleted: (id: string) => void;
}) {
  const [modalOpen, setModalOpen] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);

  async function onDelete(p: Project) {
    const hasGithubRepository = Boolean(p.githubUrl || (p.githubOwner && p.githubRepo));
    const ok = window.confirm(
      `Delete project "${p.name}"?\n\nIts experiments, runs and chats are removed from orx. ` +
        `The local folder (${p.path})${hasGithubRepository ? " and its GitHub repository" : ""} are kept.`,
    );
    if (!ok) return;
    setDeleting(p.id);
    try {
      await deleteProject(p.id);
      onDeleted(p.id);
    } catch (err) {
      window.alert(err instanceof Error ? err.message : String(err));
    } finally {
      setDeleting(null);
    }
  }

  return (
    <div className="home [flex:1] [min-height:0] [overflow-y:auto] [scrollbar-gutter:stable_both-edges] [background:var(--canvas)]">
      <div className="home-inner [max-width:620px] [margin:0_auto] [padding:48px_24px_64px]">
        <div className="home-brand [font-weight:var(--fw-display)] [font-size:var(--fs-4xl)] [letter-spacing:-0.02em] [margin-bottom:36px]">
          <Wordmark />
        </div>
        <div className="home-head [display:flex] [align-items:center] [justify-content:space-between] [gap:12px] [margin-bottom:18px] [&_h2]:[margin:0] [&_h2]:[font-size:var(--fs-2xl)] [&_h2]:[letter-spacing:-0.01em]">
          <h2>Projects</h2>
          <button className={SMALL_BUTTON_CLASS_NAME} onClick={() => setModalOpen(true)}>
            <Plus size={13} /> New project
          </button>
        </div>
        <div className="home-list [display:flex] [flex-direction:column] [gap:10px]">
          {projects.length === 0 ? (
            <div className="changes-note [font-size:var(--fs-sm)] [color:var(--muted)]">No projects yet — create one to get started.</div>
          ) : (
            [...projects].sort((a, b) => b.updatedAt - a.updatedAt).map((p) => {
              const hasGithubRepository = Boolean(p.githubUrl || (p.githubOwner && p.githubRepo));
              const githubUrl =
                p.githubUrl ??
                (p.githubOwner && p.githubRepo
                  ? `https://github.com/${p.githubOwner}/${p.githubRepo}`
                  : null);
              const githubState = p.githubEnabled
                ? "GitHub syncing on"
                : hasGithubRepository
                  ? "GitHub syncing off"
                  : "local only";
              return (
              <div
                key={p.id}
                className="project-card [position:relative] [display:flex] [flex-direction:column] [gap:4px] [text-align:left] [padding:14px_16px] [background:var(--base)] [border:1px_solid_var(--border)] [border-radius:var(--radius-lg)] [cursor:pointer] [transition:box-shadow_120ms_ease] [&:hover]:[box-shadow:0_3px_12px_color-mix(in_srgb,_var(--text)_8%,_transparent)] [&_>_:not(.project-card-open,_.project-delete)]:[position:relative] [&_>_:not(.project-card-open,_.project-delete)]:[z-index:1] [&_>_:not(.project-card-open,_.project-delete)]:[pointer-events:none] [&_.project-delete]:[position:absolute] [&_.project-delete]:[top:10px] [&_.project-delete]:[right:10px] [&_.project-delete]:[z-index:2] [&_.project-delete]:[display:none] [&_.project-delete]:[padding:5px] [&_.project-delete]:[border-radius:var(--radius-sm)] [&_.project-delete]:[color:var(--muted)] [&_.project-delete]:[line-height:0] [&:is(:hover,_:focus-within)_.project-delete]:[display:block] [&_.name]:[font-weight:var(--fw-semibold)] [&_.name]:[font-size:var(--fs-base)] [&_.paper]:[font-size:var(--fs-xs)] [&_.paper]:[color:var(--muted)] [&_.time]:[font-size:var(--fs-xs)] [&_.time]:[color:var(--muted)]"
              >
                <button
                  className="project-card-open [position:absolute] [inset:0] [border-radius:inherit] [&:focus-visible]:[outline:2px_solid_var(--text)] [&:focus-visible]:[outline-offset:2px]"
                  aria-label={`Open ${p.name}`}
                  onClick={() => onOpen(p.id)}
                />
                <span className="name">{p.name}</span>
                <span className={`project-card-sync [&_a]:[pointer-events:auto] [display:inline-flex] [align-items:center] [align-self:flex-start] [gap:7px] [color:var(--text)] [&_a]:[display:inline-flex] [&_a]:[color:var(--subtext)] [&_a:hover]:[color:var(--text)] ${MONO_CLASS_NAME}`}>
                  {githubState}
                  {p.githubEnabled && githubUrl && (
                    <a
                      href={githubUrl}
                      target="_blank"
                      rel="noreferrer"
                      data-tip="Open repository on GitHub"
                      aria-label={`Open ${p.name} on GitHub`}
                      onClick={(event) => event.stopPropagation()}
                    >
                      <GitHubMark size={14} />
                    </a>
                  )}
                </span>
                {p.paperId && <span className={`paper ${MONO_CLASS_NAME}`}>arXiv {p.paperId}</span>}
                <span className="time">created {timeAgo(p.createdAt)}</span>
                <button
                  className="project-delete [&:hover]:[color:var(--danger,_#d33)] [&:hover]:[background:var(--overlay,_rgba(0,_0,_0,_0.05))]"
                  data-tip={`Delete ${p.name}`}
                  data-tip-align="end"
                  aria-label={`Delete ${p.name}`}
                  disabled={deleting === p.id}
                  onClick={(e) => {
                    e.stopPropagation();
                    onDelete(p);
                  }}
                >
                  <Trash2 size={14} />
                </button>
              </div>
              );
            })
          )}
        </div>
      </div>

      {modalOpen && (
        <NewProjectDialog
          onClose={() => setModalOpen(false)}
          onCreated={(project, githubPublicationError) => {
            setModalOpen(false);
            onCreated(project, githubPublicationError);
          }}
        />
      )}
    </div>
  );
}
