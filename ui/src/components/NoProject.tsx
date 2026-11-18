import { useContext, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { FolderPlus, MessageCircleDashed, Plus, X } from "lucide-react";

import type { ChatSession, Project } from "../api";
import type { DialogContentProps } from "../dialog";
import { autoDir, ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { mutMoveChatToProject } from "../queries/projects";
import { useWorkspace } from "../workspace";
import { WorkspaceDomContext } from "../WorkspaceDom";
import { defaultProjectPath, useDebouncedValue } from "./NewProjectForm";
import { Button, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, IconButton, Input, Spinner, Tooltip, showAlert } from "./ui";

export type ProjectProposal = { name: string; path: string };

/** What the app calls a project: chats without one read "No project". */
export const projectLabel = (project: Pick<Project, "name" | "scratch">) => project.scratch ? m.no_project() : project.name;

/** The project `orx project create --name … [--path …]` proposes, from its argv after `orx`. */
export function projectProposal(argv: readonly string[] | null): ProjectProposal | null {
  if (argv?.[0] !== "project" || argv[1] !== "create") return null;
  const flag = (name: string) => {
    const index = argv.findIndex((token) => token === name || token.startsWith(`${name}=`));
    if (index < 0) return undefined;
    return argv[index].startsWith(`${name}=`) ? argv[index].slice(name.length + 1) : argv[index + 1];
  };
  const name = flag("--name")?.trim();
  if (!name) return null;
  return { name, path: flag("--path")?.trim() || defaultProjectPath(name) };
}

/** Name a chat without a project and choose the folder its code moves to. */
function CreateProjectDialog({ confirm, cancel, cancelable, isDisabled, isPending, errorMessage, initial, title, description }: DialogContentProps<ProjectProposal> & {
  initial?: Partial<ProjectProposal>;
  title: string;
  description: string;
}) {
  const { id: elementId } = useContext(WorkspaceDomContext);
  const { getProjectPathStatusQuery } = useWorkspace().queries;

  const [name, setName] = useState(initial?.name ?? "");
  const [typedPath, setTypedPath] = useState(initial?.path ?? null);
  const path = typedPath ?? defaultProjectPath(name);

  const checkedPath = useDebouncedValue(path.trim(), 200);
  const status = useQuery({ ...getProjectPathStatusQuery(checkedPath), enabled: Boolean(checkedPath) && checkedPath === path.trim() });
  const current = checkedPath === path.trim() ? status.data : undefined;
  const pathError = current?.exists && current.directory === false ? m.new_project_destination_is_file()
    : current?.exists && current.empty === false ? m.create_project_folder_has_files()
      : status.error?.message ?? null;
  const canSubmit = Boolean(name.trim() && path.trim()) && !pathError;

  return (
    <DialogContent
      className="w-lg"
      render={<form action="#" onSubmit={(event) => {
        event.preventDefault();
        if (canSubmit) confirm({ name: name.trim(), path: path.trim() });
      }} />}
    >
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
      </DialogHeader>
      <div className="mt-4 flex flex-col gap-3">
        <label className="flex flex-col gap-1.5 text-sm font-medium">
          {m.new_project_form_project_name()}
          <Input value={name} disabled={isDisabled} placeholder={m.new_project_form_my_research()}
            onChange={(event) => setName(event.target.value)} />
        </label>
        <label className="flex flex-col gap-1.5 text-sm font-medium">
          {m.new_project_form_project_location()}
          <Input value={path} disabled={isDisabled} placeholder="~/OpenResearch/my-research" spellCheck={false} dir="ltr"
            aria-invalid={pathError ? true : undefined}
            aria-describedby={pathError ? elementId("create-project-path-error") : undefined}
            onChange={(event) => setTypedPath(event.target.value)} />
          {pathError && <span id={elementId("create-project-path-error")} role="alert" className="text-sm font-normal text-accent-red">{pathError}</span>}
        </label>
      </div>
      {errorMessage != null && <p role="alert" className="mt-3 mb-0 text-sm text-accent-red">{errorMessage}</p>}
      <DialogFooter>
        {cancelable && <Button type="button" onClick={cancel} disabled={isDisabled}>{m.common_cancel()}</Button>}
        <Button type="submit" variant="primary" disabled={!canSubmit || isDisabled}>
          {isPending && <Spinner />}
          {m.create_project_confirm()}
        </Button>
      </DialogFooter>
    </DialogContent>
  );
}

/** Opens the dialog that makes `session` a project of its own: a chat without a
 * project becomes one, and a chat in `project` moves into a new one. */
export function useCreateProject() {
  const workspace = useWorkspace();
  return (project: Project, session: Pick<ChatSession, "id">, initial?: Partial<ProjectProposal>) =>
    workspace.dialogs.showCustomDialog({
      component: CreateProjectDialog,
      props: {
        initial,
        title: project.scratch ? m.create_project_title() : m.create_project_move_title(),
        description: project.scratch ? m.create_project_folder_description() : m.create_project_split_description({ project: autoDir(project.name) }),
      },
      onConfirm: async ({ name, path }) => {
        await mutMoveChatToProject.runAsHeadlessPromise(workspace, session.id, { name, path }, {});
      },
      onCancel: "close",
    });
}

/** The move waiting for the chat's turn to end, with a way to call it off. */
export function PendingProjectMove({ session }: { session: ChatSession }) {
  const { cancelChatProjectMove } = useWorkspace().api;
  if (!session.pendingProject) return null;
  return (
    <span className="flex min-w-0 items-center gap-1 rounded-md border border-border bg-surface ps-2.5 text-sm text-subtext">
      <span className="min-w-0 truncate">{m.create_project_pending({ name: autoDir(session.pendingProject.name) })}</span>
      <Tooltip interactive content={m.create_project_pending_cancel()} className="rounded-sm">
        <IconButton size="small" aria-label={m.create_project_pending_cancel()}
          onClick={() => void cancelChatProjectMove(session.id).catch((error: unknown) => showAlert(error instanceof Error ? error.message : String(error), "error"))}>
          <X size={14} />
        </IconButton>
      </Tooltip>
    </span>
  );
}

/** An agent's `orx project create`: the project it proposes, until the user creates it. */
export function ProjectProposalCard({ proposal, proposedAt, project, session }: {
  proposal: ProjectProposal;
  /** When the agent proposed it; a project made since then is this one. */
  proposedAt: number;
  project: Project | undefined;
  session: ChatSession | undefined;
}) {
  const createProject = useCreateProject();
  const created = project && !project.scratch && project.createdAt >= proposedAt;
  return (
    <div className="flex w-full items-start gap-3 rounded-lg border border-border bg-background px-3 py-2.5">
      <span className="flex h-6 shrink-0 items-center text-subtext"><FolderPlus size={16} aria-hidden="true" /></span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="truncate text-base font-medium text-text">{autoDir(created ? project.name : proposal.name)}</span>
        <span className="truncate text-sm text-subtext" dir="ltr">{ltr(created ? project.path : proposal.path)}</span>
      </span>
      {created ? <span className="flex h-6 shrink-0 items-center text-sm text-subtext">{m.create_project_created()}</span>
        : session?.pendingProject ? <PendingProjectMove session={session} />
          : project && session && <Button size="small" className="shrink-0" onClick={() => createProject(project, session, proposal)}>{m.create_project_review()}</Button>}
    </div>
  );
}

/** The sidebar's heading over every chat without a project. */
export function NoProjectGroupHeader({ collapsed, busy, onToggleCollapsed, onNewChat }: {
  collapsed: boolean;
  busy: boolean;
  onToggleCollapsed: () => void;
  onNewChat: () => void;
}) {
  return <>
    <button type="button" data-sidebar-keyboard-row aria-expanded={!collapsed} onClick={onToggleCollapsed}
      className="flex h-7 min-w-0 flex-1 cursor-pointer items-center gap-2 rounded-sm py-1 text-start text-sm font-normal text-text focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2">
      <MessageCircleDashed size={16} className="shrink-0" /><span className="truncate">{m.no_project()}</span>
      {busy && <span aria-hidden className="size-[7px] shrink-0 rounded-full bg-accent-teal animate-[or-pulse_1.2s_infinite]" />}
    </button>
    <Tooltip interactive content={m.no_project_new_chat()} className="rounded-sm">
      <IconButton size="small" className="text-text opacity-0 group-hover:opacity-100 group-focus-within:opacity-100"
        aria-label={m.no_project_new_chat()} onClick={onNewChat}><Plus size={16} /></IconButton>
    </Tooltip>
  </>;
}
