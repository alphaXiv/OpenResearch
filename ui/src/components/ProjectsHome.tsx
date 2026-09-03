import { m } from "../paraglide/messages.js";
import { autoDir, ltr } from "../i18n";
import { Plus, Trash2 } from "lucide-react";
import { GitHubMark } from "./BackendLogos";
import { useEffect, useRef, useState } from "react";
import {
  deleteProject,
  fmtNumber,
  listProjectActivity,
  timeAgo,
  type Project,
  type ProjectActivity,
} from "../api";
import { onProjectActivityEvent } from "../events";
import { NewProjectForm } from "./NewProjectForm";
import { Button } from "./ui";

export function NewProjectDialog({
  onClose,
  onCreated,
  remote = false,
}: {
  onClose: () => void;
  onCreated: (project: Project, githubPublicationError: string | null) => void;
  remote?: boolean;
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
    (dialog.querySelector<HTMLElement>("[data-initial-focus]") ?? focusable()[0] ?? dialog).focus();

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
      className="modal-backdrop fixed inset-0 bg-modal-backdrop flex items-start justify-center p-5 [--new-project-modal-top:clamp(4rem,20vh,24rem)] pt-[var(--new-project-modal-top)] overflow-y-auto z-100"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="modal w-120 max-w-full max-h-[calc(100vh_-_var(--new-project-modal-top)_-_1.25rem)] overflow-y-auto bg-background border border-border rounded-xl shadow-modal p-6 [&_h2]:mt-0 [&_h2]:mx-0 [&_h2]:mb-3.5 [&_h2]:text-xl [&_h2]:font-medium"
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-project-dialog-title"
        tabIndex={-1}
      >
        <h2 id="new-project-dialog-title">{m.projects_home_new_project()}</h2>
        <NewProjectForm onCancel={onClose} onCreated={onCreated} remote={remote} />
      </div>
    </div>
  );
}

function DeleteProjectDialog({
  project,
  deleting,
  error,
  onClose,
  onConfirm,
}: {
  project: Project;
  deleting: boolean;
  error: string | null;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  const deletingRef = useRef(deleting);
  onCloseRef.current = onClose;
  deletingRef.current = deleting;

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const focusable = () =>
      [...dialog.querySelectorAll<HTMLElement>('button:not([disabled]), [tabindex]:not([tabindex="-1"])')];
    (focusable()[0] ?? dialog).focus();

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        if (!deletingRef.current) onCloseRef.current();
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

  const hasSyncedRepository = Boolean(
    project.githubEnabled && (project.githubUrl || (project.githubOwner && project.githubRepo)),
  );

  return (
    <div
      className="modal-backdrop fixed inset-0 bg-modal-backdrop flex items-center justify-center p-5 overflow-y-auto z-100"
      onClick={(event) => {
        if (!deleting && event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="modal w-110 max-w-full bg-background border border-border rounded-xl shadow-modal p-6"
        role="dialog"
        aria-modal="true"
        aria-labelledby="delete-project-dialog-title"
        aria-describedby="delete-project-dialog-description"
        tabIndex={-1}
      >
        <h2 id="delete-project-dialog-title" className="mt-0 mb-3 text-xl">{m.projects_home_delete_project()}</h2>
        <div id="delete-project-dialog-description" className="flex flex-col gap-2 text-sm leading-normal text-subtext">
          <p className="m-0">
            {m.projects_delete_from_app({ name: autoDir(project.name) })}
          </p>
          <p className="m-0">
            {hasSyncedRepository ? m.projects_home_local_and_github_kept() : m.projects_home_local_folder_kept()}
          </p>
          {error && <p className="m-0 text-accent-red" role="alert">{error}</p>}
        </div>
        <div className="mt-5 flex justify-end gap-2">
          <Button disabled={deleting} onClick={onClose}>{m.projects_home_cancel()}</Button>
          <Button variant="danger" disabled={deleting} onClick={onConfirm}>
            {deleting ? m.projects_home_deleting() : m.projects_home_delete_project_action()}
          </Button>
        </div>
      </div>
    </div>
  );
}

function LiveDot() {
  return (
    <span className="activity-pulse h-2 w-2 shrink-0 rounded-full bg-accent-teal animate-[or-pulse_1.2s_ease-in-out_infinite]" />
  );
}

export function ProjectsHome({
  projects,
  onOpen,
  onCreated,
  onDeleted,
  remote = false,
}: {
  projects: Project[];
  onOpen: (id: string) => void;
  onCreated: (project: Project, githubPublicationError: string | null) => void;
  onDeleted: (id: string) => void;
  remote?: boolean;
}) {
  const [modalOpen, setModalOpen] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [projectPendingDelete, setProjectPendingDelete] = useState<Project | null>(null);
  const [activityByProject, setActivityByProject] = useState<Record<string, ProjectActivity>>({});
  const activityRequestRef = useRef(0);
  const activityKey = projects.map((project) => project.id).join("\u0000");

  useEffect(() => {
    let current = true;
    let refreshTimer: ReturnType<typeof setTimeout> | null = null;

    const refresh = () => {
      refreshTimer = null;
      const requestId = ++activityRequestRef.current;
      void listProjectActivity()
        .then((activity) => {
          if (!current || requestId !== activityRequestRef.current) return;
          setActivityByProject(
            Object.fromEntries(activity.map((summary) => [summary.projectId, summary])),
          );
        })
        .catch(() => {});
    };
    const scheduleRefresh = () => {
      if (refreshTimer !== null) return;
      refreshTimer = setTimeout(refresh, 100);
    };

    refresh();
    const unsubscribe = onProjectActivityEvent(scheduleRefresh);
    return () => {
      current = false;
      unsubscribe();
      if (refreshTimer !== null) clearTimeout(refreshTimer);
    };
  }, [activityKey]);

  async function onDelete(p: Project) {
    setDeleting(p.id);
    setDeleteError(null);
    try {
      await deleteProject(p.id);
      setDeleteError(null);
      setProjectPendingDelete(null);
      onDeleted(p.id);
    } catch (err) {
      setDeleteError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeleting(null);
    }
  }

  return (
    <div className="home flex-1 min-h-0 overflow-y-auto [scrollbar-gutter:stable_both-edges] bg-canvas">
      <div className="home-inner max-w-290 my-0 mx-auto pt-12 px-6 pb-16 [@media((max-width:_960px))]:pt-6 [@media((max-width:_960px))]:px-4">
        <div className="home-head flex items-center justify-between gap-3 mb-4.5 [&_h2]:m-0 [&_h2]:text-4xl [&_h2]:tracking-[-0.02em] [@media((max-width:_520px))]:items-start [@media((max-width:_520px))]:flex-col">
          <h2>{m.projects_home_projects()}</h2>
          <Button
            onClick={() => setModalOpen(true)}
          >
            <Plus size={15} /> {m.projects_home_new_project()}
          </Button>
        </div>
        <div className="home-list overflow-hidden rounded-lg border border-border bg-background">
          <div>
            <div className="grid grid-cols-[minmax(0,1fr)_9rem_9rem_minmax(18rem,max-content)] items-center gap-3 border-b border-border bg-background py-2.5 ps-4 pe-2 text-xs font-medium tracking-[0.06em] text-text uppercase [@media((max-width:_960px))]:hidden">
              <span>{m.projects_home_project()}</span>
              <span>{m.projects_home_agents()}</span>
              <span>{m.projects_home_experiments()}</span>
              <span>{m.projects_home_repository()}</span>
            </div>
            {projects.length === 0 ? (
              <div className="py-8 px-4 text-sm text-muted">{m.projects_home_no_projects_yet_create_one_to_get_started()}</div>
            ) : (
              [...projects].sort((a, b) => {
                const aActivity = activityByProject[a.id]?.lastMessageAt ?? a.createdAt;
                const bActivity = activityByProject[b.id]?.lastMessageAt ?? b.createdAt;
                return bActivity - aActivity || a.name.localeCompare(b.name);
              }).map((p) => {
                const summary = activityByProject[p.id];
                const githubUrl = p.githubEnabled
                  ? p.githubUrl ??
                    (p.githubOwner && p.githubRepo
                      ? `https://github.com/${p.githubOwner}/${p.githubRepo}`
                      : null)
                  : null;
                const githubState = githubUrl
                  ? p.githubOwner && p.githubRepo
                    ? `${p.githubOwner}/${p.githubRepo}`
                    : githubUrl
                        .replace(/^https?:\/\/github\.com\//, "")
                        .replace(/\.git$/, "")
                        .replace(/\/$/, "")
                  : m.projects_local();
                const agentsLabel = summary
                  ? summary.activeAgents > 0
                    ? m.projects_active_count({ count: fmtNumber(summary.activeAgents) })
                    : m.projects_idle()
                  : "—";
                const agentTotal = summary
                  ? summary.totalAgents === 1 ? m.projects_one_agent() : m.projects_agents_count({ count: fmtNumber(summary.totalAgents) })
                  : "—";
                const experimentLabel = !summary
                  ? "—"
                  : summary.runningExperiments > 0
                    ? m.projects_running_count({ count: fmtNumber(summary.runningExperiments) })
                    : summary.totalExperiments === 0
                      ? m.settings_none()
                      : m.projects_total_count({ count: fmtNumber(summary.totalExperiments) });
                const experimentTotal =
                  summary && summary.runningExperiments > 0
                    ? m.projects_total_count({ count: fmtNumber(summary.totalExperiments) })
                    : null;
                return (
                  <div
                    key={p.id}
                    className="group project-row relative grid cursor-pointer grid-cols-[minmax(0,1fr)_9rem_9rem_minmax(18rem,max-content)] items-center gap-3 border-b border-border-variant py-4 ps-4 pe-2 text-start transition-colors duration-120 ease-standard last:border-b-0 hover:bg-surface-bright focus-within:bg-surface-bright [@media((max-width:_960px))]:grid-cols-[minmax(0,0.8fr)_minmax(0,0.8fr)_minmax(0,1.4fr)] [@media((max-width:_960px))]:items-start [@media((max-width:_960px))]:gap-x-4 [@media((max-width:_960px))]:gap-y-3 [@media((max-width:_960px))]:py-4 [@media((max-width:_960px))]:px-4 [@media((max-width:_600px))]:grid-cols-2"
                  >
                    <button
                      className="project-row-open absolute inset-0 z-0 cursor-pointer rounded-[inherit] focus-visible:outline focus-visible:outline-2 focus-visible:outline-text focus-visible:outline-offset-[-2px]"
                      aria-label={m.a11y_open_item({ name: autoDir(p.name) })}
                      onClick={() => onOpen(p.id)}
                   />
                    {/* Cells stay click-transparent so the stretched button owns row navigation. */}
                    <div className="relative z-1 flex min-w-0 flex-col gap-1 pointer-events-none [@media((max-width:_960px))]:col-span-3 [@media((max-width:_600px))]:col-span-2">
                      <span dir="auto" className="project-row-title whitespace-normal break-words text-base font-semibold text-text pointer-events-none">{p.name}</span>
                      <span className="relative z-2 flex items-center gap-1.5 text-xs text-muted [@media((max-width:_960px))]:flex-wrap">
                        <span>{m.projects_home_created()} {timeAgo(p.createdAt)}</span>
                        {p.paperId && <span aria-hidden="true">·</span>}
                        {p.paperId && <span>{m.projects_home_ar_xiv_paper_id()} {ltr(p.paperId)}</span>}
                        <button
                          className="project-row-secondary project-row-delete inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-sm leading-0 text-muted opacity-0 pointer-events-none transition-opacity hover:bg-surface hover:text-accent-red group-hover:opacity-100 group-hover:pointer-events-auto group-focus-within:opacity-100 group-focus-within:pointer-events-auto focus:opacity-100 focus:pointer-events-auto focus-visible:outline focus-visible:outline-2 focus-visible:outline-text"
                          aria-label={m.a11y_delete_item({ name: autoDir(p.name) })}
                          disabled={deleting === p.id}
                          onClick={(event) => {
                            event.stopPropagation();
                            setDeleteError(null);
                            setProjectPendingDelete(p);
                          }}
                        >
                          <Trash2 size={14} />
                        </button>
                      </span>
                    </div>
                    <div className="relative z-1 flex min-w-0 flex-col gap-1 pointer-events-none">
                      <span className="hidden text-xs font-medium tracking-[0.06em] text-text uppercase [@media((max-width:_960px))]:block">{m.projects_home_agents()}</span>
                      <span className="inline-flex items-center gap-2 text-sm text-text">
                        {summary && summary.activeAgents > 0 && (
                          <LiveDot />
                        )}
                        {agentsLabel}
                      </span>
                      <span className="text-xs text-muted">{agentTotal}</span>
                    </div>
                    <div className="relative z-1 flex min-w-0 flex-col gap-1 pointer-events-none">
                      <span className="hidden text-xs font-medium tracking-[0.06em] text-text uppercase [@media((max-width:_960px))]:block">{m.projects_home_experiments()}</span>
                      <span className="inline-flex items-center gap-2 text-sm text-text">
                        {summary && summary.runningExperiments > 0 && (
                          <LiveDot />
                        )}
                        {experimentLabel}
                      </span>
                      {experimentTotal && <span className="text-xs text-muted">{experimentTotal}</span>}
                    </div>
                    <div className="relative z-1 min-w-0 pointer-events-none [@media((max-width:_600px))]:col-span-2">
                      <span className="hidden text-xs font-medium tracking-[0.06em] text-text uppercase [@media((max-width:_960px))]:mb-1 [@media((max-width:_960px))]:block">{m.projects_home_repository()}</span>
                      {githubUrl ? (
                        <a
                          className="project-row-secondary inline-flex max-w-full items-center gap-2 text-sm text-text no-underline pointer-events-auto hover:underline underline-offset-2"
                          href={githubUrl}
                          target="_blank"
                          rel="noreferrer"
                          aria-label={m.a11y_open_on_github({ name: autoDir(p.name) })}
                        >
                          <span className="inline-flex shrink-0"><GitHubMark size={14} /></span>
                          <span className="overflow-hidden text-ellipsis whitespace-nowrap [@media((max-width:_960px))]:whitespace-normal [@media((max-width:_960px))]:break-all">{ltr(githubState)}</span>
                        </a>
                      ) : (
                        <span className="text-sm text-text pointer-events-none">{githubState}</span>
                      )}
                    </div>
                  </div>
                );
              })
            )}
          </div>
        </div>
      </div>

      {modalOpen && (
        <NewProjectDialog
          remote={remote}
          onClose={() => setModalOpen(false)}
          onCreated={(project, githubPublicationError) => {
            setModalOpen(false);
            onCreated(project, githubPublicationError);
          }}
       />
      )}
      {projectPendingDelete && (
        <DeleteProjectDialog
          project={projectPendingDelete}
          deleting={deleting === projectPendingDelete.id}
          error={deleteError}
          onClose={() => {
            setDeleteError(null);
            setProjectPendingDelete(null);
          }}
          onConfirm={() => void onDelete(projectPendingDelete)}
       />
      )}
    </div>
  );
}
