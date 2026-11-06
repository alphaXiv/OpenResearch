import { useWorkspace } from "../workspace";
import { useQuery } from "@tanstack/react-query";

import { m } from "../paraglide/messages.js";
import { useRef, useState } from "react";
import { FileArchive, FolderOpen, Plus } from "lucide-react";
import { type AgentSelection, type Project, type SharedSource } from "../api";

import { type DialogContentProps } from "../dialog";
import { NewProjectForm } from "./NewProjectForm";
import { OpenSharedProjectForm, reportMissingBranches } from "./OpenSharedProject";
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
  const { showCustomDialog } = useWorkspace().dialogs;

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
  initialShared,
}: DialogContentProps<void> & {
  onCreated: (project: Project, githubPublicationError: string | null) => void;
  remote?: boolean;
  preferredAgent: AgentSelection | null;
  initialPaperId?: string;
  /** A shared project to open instead of creating one. */
  initialShared?: SharedSource;
}) {
  const { getUiStateQuery } = useWorkspace().queries;

  const dialogRef = useRef<HTMLDivElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const [shared, setShared] = useState<SharedSource | null>(initialShared ?? null);
  const { data: uiState } = useQuery(getUiStateQuery());

  if (shared) return <DialogContent ref={dialogRef} className="max-h-[calc(100vh-2.5rem)] overflow-y-auto">
    <DialogTitle className="mb-3.5">{m.open_shared_title()}</DialogTitle>
    <OpenSharedProjectForm source={shared} onCancel={() => initialShared ? cancel() : setShared(null)} onOpened={(opened) => {
      confirm();
      onCreated(opened.project, null);
      reportMissingBranches(opened);
    }} />
  </DialogContent>;

  return <DialogContent ref={dialogRef}
      initialFocus={() => dialogRef.current?.querySelector<HTMLElement>("[data-initial-focus]") ?? true}
      className="max-h-[calc(100vh-2.5rem)] overflow-y-auto">
      <div className="mb-3.5 flex items-center justify-between gap-2">
        <DialogTitle>{m.projects_home_new_project()}</DialogTitle>
        <Button type="button" variant="ghost" onClick={() => fileRef.current?.click()}><FileArchive size={16} />{m.open_shared_pick()}</Button>
        <input ref={fileRef} type="file" accept=".orx" hidden onChange={(event) => {
          setShared(event.target.files?.[0] ?? null);
          // Cleared so picking the same file after Cancel fires again.
          event.target.value = "";
        }} />
      </div>
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

