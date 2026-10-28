import { EscapeScope } from "./EscapeScope";
import { m } from "../paraglide/messages.js";
import { useDialogFocus } from "./useDialogFocus";
import { useRef, useState } from "react";
import { FolderOpen, Plus } from "lucide-react";
import { type AgentSelection, type Project } from "../api";

import { NewProjectForm } from "./NewProjectForm";
import { Button } from "./ui";

/** The projects home once every project is gone; onboarding must not replay here. */
export function ProjectsEmptyState({
  remote = false,
  preferredAgent,
  onCreated,
}: {
  remote?: boolean;
  preferredAgent: AgentSelection | null;
  onCreated: (project: Project, githubPublicationError: string | null) => void;
}) {
  const [dialogOpen, setDialogOpen] = useState(false);
  return (
    <div className="home flex flex-1 min-h-0 flex-col items-center justify-center gap-3 bg-background p-6 text-center">
      <FolderOpen size={36} strokeWidth={1.5} className="shrink-0 text-muted" />
      <p className="m-0 w-full max-w-105 text-lg leading-normal text-subtext">
        {m.projects_home_no_projects_yet_create_one_to_get_started()}
      </p>
      <Button variant="primary" className="mt-1" onClick={() => setDialogOpen(true)}>
        <Plus size={14} /> {m.projects_home_new_project()}
      </Button>
      {dialogOpen && (
        <NewProjectDialog
          remote={remote}
          preferredAgent={preferredAgent}
          onClose={() => setDialogOpen(false)}
          onCreated={(project, publicationError) => {
            setDialogOpen(false);
            onCreated(project, publicationError);
          }}
        />
      )}
    </div>
  );
}

export function NewProjectDialog({
  onClose,
  onCreated,
  remote = false,
  preferredAgent,
  initialPaperId,
}: {
  onClose: () => void;
  onCreated: (project: Project, githubPublicationError: string | null) => void;
  remote?: boolean;
  preferredAgent: AgentSelection | null;
  initialPaperId?: string;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  useDialogFocus(dialogRef);

  return <EscapeScope onEscape={onClose}>
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
        <NewProjectForm
          onCancel={onClose}
          onCreated={onCreated}
          remote={remote}
          preferredAgent={preferredAgent}
          initialPaperId={initialPaperId}
        />
      </div>
    </div>
  </EscapeScope>;
}

