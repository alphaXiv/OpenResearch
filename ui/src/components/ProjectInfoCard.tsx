import { showCustomDialog, type DialogContentProps } from "../dialog";
import { useFileManagerLabel } from "./useFileManagerLabel";
import { useId, useRef, useState, type RefObject } from "react";
import { Menu } from "@base-ui/react/menu";
import { DirectionProvider } from "@base-ui/react/direction-provider";
import { Folder, FolderOpen, MessageCircle, MoreHorizontal, Pin, Plus, Settings, Trash2, X } from "lucide-react";
import { deleteProject, revealFileInManager, updateProject, type Project } from "../api";
import { m } from "../paraglide/messages.js";
import { Button, DialogContent, DialogTitle, Spinner, IconButton, Input, MenuItem, Tooltip, showAlert } from "./ui";

function deleteExplanation(project: Project) {
  const synced = project.githubEnabled && (project.githubUrl || (project.githubOwner && project.githubRepo));
  return `${m.projects_delete_from_app({ name: project.name })}\n\n${synced ? m.projects_home_local_and_github_kept() : m.projects_home_local_folder_kept()}`;
}

export function ProjectInfoCard({ project, chatCount, collapsed, busy, onToggleCollapsed, onRemoved, onNewChat, pinned, onPin }: {
  project: Project;
  chatCount: number | undefined;
  collapsed: boolean;
  busy: boolean;
  onToggleCollapsed: () => void;
  onRemoved: () => void;
  onNewChat: () => void;
  pinned: boolean;
  onPin: () => void;
}) {
  const revealLabel = useFileManagerLabel();
  const [menuOpen, setMenuOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);

  function remove() {
    setMenuOpen(false);
    showCustomDialog({
      component: DeleteProjectDialog,
      props: { project, returnFocus: triggerRef },
      onCancel: "close",
      onConfirm: async (deleteArtifacts) => {
        const artifactsError = await deleteProject(project.id, deleteArtifacts);
        onRemoved();
        if (artifactsError) showAlert(artifactsError, "error");
      },
    });
  }

  return <>
    <button type="button" data-sidebar-keyboard-row aria-expanded={!collapsed} onClick={onToggleCollapsed}
      className="flex h-7 min-w-0 flex-1 cursor-pointer items-center gap-2 rounded-sm py-1 text-start text-sm font-normal text-text focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-focus focus-visible:outline-offset-2">
      {collapsed ? <Folder size={16} className="shrink-0" /> : <FolderOpen size={16} className="shrink-0" />}<span className="truncate">{project.name}</span>
      {busy && <span aria-hidden className="size-[7px] shrink-0 rounded-full bg-accent-teal animate-[or-pulse_1.2s_infinite]" />}
    </button>
    <DirectionProvider direction={typeof document !== "undefined" && document.documentElement.dir === "rtl" ? "rtl" : "ltr"}>
      <Menu.Root open={menuOpen} onOpenChange={setMenuOpen} modal={false}>
        <Menu.Trigger ref={triggerRef} render={<IconButton size="small" className={`text-text group-hover:opacity-100 group-focus-within:opacity-100 ${menuOpen ? "bg-surface opacity-100" : "opacity-0"}`} />}
          aria-label={m.sidebar_project_details({ name: project.name })}><MoreHorizontal size={16} /></Menu.Trigger>
        <Menu.Portal>
          <Menu.Positioner sticky positionMethod="fixed" side="inline-end" align="start" sideOffset={12} collisionPadding={8} className="z-100">
            <Menu.Popup aria-label={project.name}
              className="outline-none w-72 max-w-[calc(100vw-1rem)] max-h-(--available-height) overflow-y-auto rounded-xl border border-border bg-background p-2 text-text shadow-modal">
              <div role="presentation" className="mb-1 border-b border-border px-2 pb-2 pt-1 text-sm">
                <div className="truncate font-medium">{project.name}</div>
                <div className="mt-1 flex items-center gap-1.5 text-subtext"><MessageCircle size={14} />{chatCount === undefined ? "…" : m.sidebar_project_chats({ count: chatCount })}</div>
                <div className="mt-2 break-all text-xs text-subtext">{project.repoPath}</div>
              </div>
              <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={onPin}><span className="flex items-center gap-2"><Pin size={16} />{pinned ? m.sidebar_unpin_project() : m.sidebar_pin_project()}</span></Menu.Item>
              <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => {
                setMenuOpen(false);
                showCustomDialog({
                  component: EditProjectDialog,
                  props: { project, onDelete: remove, returnFocus: triggerRef },
                  onCancel: "close",
                  onConfirm: async (name) => { await updateProject(project.id, { name }); },
                });
              }}><span className="flex items-center gap-2"><Settings size={16} />{m.activity_edit()}</span></Menu.Item>
              <Menu.Item nativeButton render={<MenuItem className="data-highlighted:bg-surface" />} onClick={() => {
                void revealFileInManager(project.id, ".").catch((err: unknown) => showAlert(err instanceof Error ? err.message : String(err), "error"));
              }}><span className="flex items-center gap-2"><FolderOpen size={16} />{revealLabel}</span></Menu.Item>
              <Menu.Separator className="my-1 border-t border-border" />
              <Menu.Item nativeButton render={<MenuItem danger className="data-highlighted:bg-surface" />} onClick={remove}><span className="flex items-center gap-2"><Trash2 size={16} />{m.projects_home_delete_project_action()}</span></Menu.Item>
            </Menu.Popup>
          </Menu.Positioner>
        </Menu.Portal>
      </Menu.Root>
    </DirectionProvider>
    <Tooltip interactive content={m.chat_panel_new_chat()} className="rounded-sm">
      <IconButton size="small" className="text-text opacity-0 group-hover:opacity-100 group-focus-within:opacity-100"
        aria-label={`${m.chat_panel_new_chat()} · ${project.name}`}
        onClick={onNewChat}><Plus size={16} /></IconButton>
    </Tooltip>
  </>;
}

function DeleteProjectDialog({ project, returnFocus, cancel, confirm, isDisabled: pending, isPending, errorMessage: error }: DialogContentProps<boolean> & { project: Project; returnFocus: RefObject<HTMLButtonElement | null> }) {
  const [deleteArtifacts, setDeleteArtifacts] = useState(false);
  const cancelRef = useRef<HTMLButtonElement>(null);

  return <DialogContent finalFocus={returnFocus} initialFocus={cancelRef} className="max-h-[calc(100vh-2.5rem)] overflow-y-auto text-text">
    <DialogTitle>{m.projects_home_delete_project_action()}</DialogTitle>
    <p className="mt-4 whitespace-pre-line text-sm">{deleteExplanation(project)}</p>
    <p className="mt-4 text-sm">{m.projects_delete_artifacts_kept()}</p>
    <p className="mt-2 break-all text-xs text-subtext">{project.artifactsDir}</p>
    <label className="mt-4 flex items-start gap-2 text-sm"><input type="checkbox" checked={deleteArtifacts} disabled={pending} onChange={(event) => setDeleteArtifacts(event.target.checked)} className="mt-1" />{m.projects_delete_artifacts_remove()}</label>
    {error && <p role="alert" className="mt-3 text-sm text-accent-red">{error}</p>}
    <div className="mt-6 flex justify-end gap-2"><Button type="button" variant="ghost" ref={cancelRef} disabled={pending} onClick={cancel}>{m.projects_home_cancel()}</Button><Button type="button" variant="danger" disabled={pending} onClick={() => confirm(deleteArtifacts)}>{isPending && <Spinner />}{m.projects_home_delete_project_action()}</Button></div>
  </DialogContent>;
}

function EditProjectDialog({ project, returnFocus, cancel, confirm, isDisabled: pending, isPending, errorMessage: error, onDelete }: DialogContentProps<string> & { project: Project; onDelete: () => void; returnFocus: RefObject<HTMLButtonElement | null> }) {
  const nameId = useId();
  const nameRef = useRef<HTMLInputElement>(null);
  const [name, setName] = useState(project.name);

  return <DialogContent finalFocus={returnFocus} initialFocus={nameRef} className="max-h-[calc(100vh-2.5rem)] overflow-y-auto text-text"
    render={<form onSubmit={(event) => { event.preventDefault(); if (name.trim() && !pending) confirm(name.trim()); }} />}>
    <div className="mb-5 flex items-center justify-between"><DialogTitle>{m.sidebar_edit_project()}</DialogTitle><IconButton type="button" aria-label={m.tour_close()} onClick={cancel} disabled={pending}><X size={16} /></IconButton></div>
    <label htmlFor={nameId} className="sr-only">{m.new_project_form_project_name()}</label>
    <div className="relative"><FolderOpen size={16} className="pointer-events-none absolute start-3 top-3 text-subtext" /><Input id={nameId} ref={nameRef} value={name} onChange={(event) => setName(event.target.value)} disabled={pending} required className="h-10 ps-10" /></div>
    <div className="mt-5 mb-2 text-sm font-medium">{m.sidebar_source_folder()}</div>
    <div className="flex items-start gap-2 rounded-lg border border-border p-3 text-sm"><FolderOpen size={16} className="mt-0.5 shrink-0 text-subtext" /><span className="min-w-0 break-all">{project.repoPath}</span></div>
    {error && <p role="alert" className="mt-3 text-sm text-accent-red">{error}</p>}
    <div className="mt-6 flex items-center gap-2"><Button type="button" variant="danger" disabled={pending} onClick={() => { cancel(); onDelete(); }}>{m.projects_home_delete_project_action()}</Button><div className="flex-1" /><Button type="button" variant="ghost" disabled={pending} onClick={cancel}>{m.projects_home_cancel()}</Button><Button type="submit" variant="primary" disabled={pending || !name.trim()}>{isPending && <Spinner />}{m.common_save()}</Button></div>
  </DialogContent>;
}
