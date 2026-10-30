import { useQuery } from "@tanstack/react-query";
import { getUiStateQuery } from "../queries/projects";
import { m } from "../paraglide/messages.js";
import { useRef } from "react";
import { FolderOpen, Plus } from "lucide-react";
import { type AgentSelection, type Project } from "../api";

import { showCustomDialog, type DialogContentProps } from "../dialog";
import { NewProjectForm } from "./NewProjectForm";
import { Button, DialogContent, DialogTitle } from "./ui";

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
  return (
    <div className="home flex flex-1 min-h-0 flex-col items-center justify-center gap-3 bg-background p-6 text-center">
      <FolderOpen size={36} strokeWidth={1.5} className="shrink-0 text-muted" />
      <p className="m-0 w-full max-w-105 text-lg leading-normal text-subtext">
        {m.projects_home_no_projects_yet_create_one_to_get_started()}
      </p>
      <Button variant="primary" className="mt-1" onClick={() => showCustomDialog({
        component: NewProjectDialog,
        props: { remote, preferredAgent, onCreated },
        onConfirm: () => {},
        onCancel: "close",
      })}>
        <Plus size={14} /> {m.projects_home_new_project()}
      </Button>
    </div>
  );
}

export function NewProjectDialog({
  cancel,
  confirm,
  onCreated,
  remote = false,
  preferredAgent,
  initialPaperId,
}: DialogContentProps<void> & {
  onCreated: (project: Project, githubPublicationError: string | null) => void;
  remote?: boolean;
  preferredAgent: AgentSelection | null;
  initialPaperId?: string;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const { data: uiState } = useQuery(getUiStateQuery());

  return <DialogContent ref={dialogRef}
      initialFocus={() => dialogRef.current?.querySelector<HTMLElement>("[data-initial-focus]") ?? true}
      className="max-h-[calc(100vh-2.5rem)] overflow-y-auto">
      <DialogTitle className="mb-3.5">{m.projects_home_new_project()}</DialogTitle>
      <NewProjectForm
        onCancel={cancel}
        onCreated={(project, publicationError) => {
          confirm();
          onCreated(project, publicationError);
        }}
        remote={remote}
        preferredAgent={uiState ? uiState.preferredAgent : preferredAgent}
        initialPaperId={initialPaperId}
      />
    </DialogContent>;
}

