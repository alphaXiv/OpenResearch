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
      className="modal-backdrop fixed inset-0 bg-[rgba(29,_27,_26,_0.4)] flex items-start justify-center pt-[var(--modal-top)] px-4 pb-6 overflow-y-auto z-100"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="modal w-120 max-w-[94vw] max-h-[calc(100vh_-_var(--modal-top)_-_48px)] overflow-y-auto bg-background border border-border rounded-xl shadow-[0_24px_60px_rgba(0,_0,_0,_0.22)] p-6 [&_h2]:mt-0 [&_h2]:mx-0 [&_h2]:mb-3.5 [&_h2]:text-xl"
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
    <div className="home flex-1 min-h-0 overflow-y-auto [scrollbar-gutter:stable_both-edges] bg-canvas">
      <div className="home-inner max-w-155 my-0 mx-auto pt-12 px-6 pb-16">
        <div className="home-brand font-display text-4xl tracking-[-0.02em] mb-9">
          <Wordmark />
        </div>
        <div className="home-head flex items-center justify-between gap-3 mb-4.5 [&_h2]:m-0 [&_h2]:text-2xl [&_h2]:tracking-[-0.01em]">
          <h2>Projects</h2>
          <button className={SMALL_BUTTON_CLASS_NAME} onClick={() => setModalOpen(true)}>
            <Plus size={13} /> New project
          </button>
        </div>
        <div className="home-list flex flex-col gap-2.5">
          {projects.length === 0 ? (
            <div className="changes-note text-sm text-muted">No projects yet — create one to get started.</div>
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
                className="project-card relative flex flex-col gap-1 text-left py-3.5 px-4 bg-background border border-border rounded-lg cursor-pointer transition-[box-shadow] duration-120 ease-standard [&:hover]:shadow-[0_3px_12px_color-mix(in_srgb,_var(--text)_8%,_transparent)] [&_>_:not(.project-card-open,_.project-delete)]:relative [&_>_:not(.project-card-open,_.project-delete)]:z-1 [&_>_:not(.project-card-open,_.project-delete)]:pointer-events-none [&_.project-delete]:absolute [&_.project-delete]:top-2.5 [&_.project-delete]:right-2.5 [&_.project-delete]:z-2 [&_.project-delete]:hidden [&_.project-delete]:p-[5px] [&_.project-delete]:rounded-sm [&_.project-delete]:text-muted [&_.project-delete]:leading-[0] [&:is(:hover,_:focus-within)_.project-delete]:block [&_.name]:font-semibold [&_.name]:text-base [&_.paper]:text-xs [&_.paper]:text-muted [&_.time]:text-xs [&_.time]:text-muted"
              >
                <button
                  className="project-card-open absolute inset-0 rounded-[inherit] [&:focus-visible]:outline-2 [&:focus-visible]:outline-solid [&:focus-visible]:outline-text [&:focus-visible]:outline-offset-2"
                  aria-label={`Open ${p.name}`}
                  onClick={() => onOpen(p.id)}
                />
                <span className="name">{p.name}</span>
                <span className={`project-card-sync [&_a]:pointer-events-auto inline-flex items-center self-start gap-[7px] text-text [&_a]:inline-flex [&_a]:text-subtext [&_a:hover]:text-text ${MONO_CLASS_NAME}`}>
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
                  className="project-delete [&:hover]:text-[var(--danger,_#d33)] [&:hover]:bg-[var(--overlay,_rgba(0,_0,_0,_0.05))]"
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
