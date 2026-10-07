import { useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Folder, FolderOpen, MessageCircle, MoreHorizontal, Pin, Plus, Settings, Trash2, X } from "lucide-react";
import { deleteProject, revealFileInManager, updateProject, type Project } from "../api";
import { m } from "../paraglide/messages.js";
import { Button, IconButton, Input, MenuItem, Tooltip, showAlert } from "./ui";
import { useDialogFocus } from "./useDialogFocus";

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
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuId = useId();
  const [menuOpen, setMenuOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  useEffect(() => {
    const dismiss = (event: Event) => {
      if (event.target instanceof Node && menuRef.current?.contains(event.target)) return;
      menuRef.current?.hidePopover();
    };
    window.addEventListener("scroll", dismiss, true);
    window.addEventListener("resize", dismiss);
    return () => {
      window.removeEventListener("scroll", dismiss, true);
      window.removeEventListener("resize", dismiss);
    };
  }, []);

  function show(popover: HTMLDivElement | null, trigger: HTMLButtonElement | null) {
    if (!popover || !trigger) return;
    popover.showPopover();
    const anchor = trigger.getBoundingClientRect();
    const bounds = popover.getBoundingClientRect();
    const left = anchor.right + 12 + bounds.width <= window.innerWidth - 8
      ? anchor.right + 12 : Math.max(8, anchor.left - bounds.width - 12);
    popover.style.left = `${left}px`;
    popover.style.top = `${Math.max(8, Math.min(anchor.top, window.innerHeight - bounds.height - 8))}px`;
  }
  function edit() {
    menuRef.current?.hidePopover();
    setEditing(true);
  }
  async function remove() {
    menuRef.current?.hidePopover();
    if (deleting || !window.confirm(deleteExplanation(project))) return;
    setDeleting(true);
    try { await deleteProject(project.id); onRemoved(); }
    catch (err) { showAlert(err instanceof Error ? err.message : String(err), "error"); }
    finally { setDeleting(false); }
  }

  return <>
    <button type="button" aria-expanded={!collapsed} onClick={onToggleCollapsed}
      className="flex h-7 min-w-0 flex-1 cursor-pointer items-center gap-[7px] rounded-sm py-1 text-start text-sm font-normal text-text focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-text focus-visible:outline-offset-2">
      {collapsed ? <Folder size={15} className="shrink-0" /> : <FolderOpen size={15} className="shrink-0" />}<span className="truncate">{project.name}</span>
      {busy && <span aria-hidden className="size-[7px] shrink-0 rounded-full bg-primary animate-[or-pulse_1.2s_infinite]" />}
    </button>
    <IconButton ref={triggerRef} size="small" className={`text-text group-hover:opacity-100 group-focus-within:opacity-100 ${menuOpen ? "bg-surface opacity-100" : "opacity-0"}`}
      aria-label={m.sidebar_project_details({ name: project.name })} aria-controls={menuId} aria-haspopup="menu" aria-expanded={menuOpen} onClick={() => {
          if (menuRef.current?.matches(":popover-open")) menuRef.current.hidePopover();
        else { show(menuRef.current, triggerRef.current); menuRef.current?.querySelector<HTMLButtonElement>("button")?.focus(); }
      }}><MoreHorizontal size={15} /></IconButton>
    <Tooltip interactive content={m.chat_panel_new_chat()} className="rounded-sm">
      <IconButton size="small" className="text-text opacity-0 group-hover:opacity-100 group-focus-within:opacity-100"
        aria-label={`${m.chat_panel_new_chat()} · ${project.name}`}
        onClick={onNewChat}><Plus size={15} /></IconButton>
    </Tooltip>
    <div ref={menuRef} id={menuId} popover="auto" role="menu" aria-label={project.name}
      onToggle={(event) => setMenuOpen(event.newState === "open")}
      onKeyDown={(event) => {
        if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
        event.preventDefault();
        const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
        const index = buttons.findIndex((button) => button === document.activeElement);
        buttons[(index + (event.key === "ArrowDown" ? 1 : buttons.length - 1)) % buttons.length]?.focus();
      }}
      className="fixed inset-auto m-0 w-72 max-w-[calc(100vw-1rem)] rounded-xl border border-border bg-background p-2 text-text shadow-modal">
      <div role="presentation" className="mb-1 border-b border-border px-2 pb-2 pt-1 text-sm">
        <div className="truncate font-medium">{project.name}</div>
        <div className="mt-1 flex items-center gap-1.5 text-subtext"><MessageCircle size={13} />{chatCount === undefined ? "…" : m.sidebar_project_chats({ count: chatCount })}</div>
        <div className="mt-2 break-all text-xs text-subtext">{project.repoPath}</div>
      </div>
      <MenuItem role="menuitem" onClick={() => { onPin(); menuRef.current?.hidePopover(); }}><span className="flex items-center gap-2"><Pin size={16} />{pinned ? m.sidebar_unpin_project() : m.sidebar_pin_project()}</span></MenuItem>
      <MenuItem role="menuitem" onClick={edit}><span className="flex items-center gap-2"><Settings size={16} />{m.activity_edit()}</span></MenuItem>
      <MenuItem role="menuitem" onClick={() => {
        menuRef.current?.hidePopover();
        void revealFileInManager(project.id, ".").catch((err: unknown) => showAlert(err instanceof Error ? err.message : String(err), "error"));
      }}><span className="flex items-center gap-2"><FolderOpen size={16} />{m.sidebar_reveal_project()}</span></MenuItem>
      <div className="my-1 border-t border-border" />
      <MenuItem role="menuitem" danger disabled={deleting} onClick={() => void remove()}><span className="flex items-center gap-2"><Trash2 size={16} />{m.projects_home_delete_project_action()}</span></MenuItem>
    </div>
    {editing && createPortal(<EditProjectDialog project={project} onClose={() => setEditing(false)} onRemoved={onRemoved} />, document.body)}
  </>;
}

function EditProjectDialog({ project, onClose, onRemoved }: { project: Project; onClose: () => void; onRemoved: () => void }) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const nameId = useId();
  const [name, setName] = useState(project.name);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const close = () => { if (!pending) onClose(); };
  useDialogFocus(dialogRef, close);

  async function save() {
    if (pending || !name.trim()) return;
    setPending(true);
    setError("");
    try { await updateProject(project.id, { name: name.trim() }); onClose(); }
    catch (err) { setError(err instanceof Error ? err.message : String(err)); }
    finally { setPending(false); }
  }

  async function remove() {
    if (pending || !window.confirm(deleteExplanation(project))) return;
    setPending(true);
    setError("");
    try { await deleteProject(project.id); onClose(); onRemoved(); }
    catch (err) { setError(err instanceof Error ? err.message : String(err)); }
    finally { setPending(false); }
  }

  return <div className="modal-backdrop fixed inset-0 z-100 flex items-center justify-center bg-modal-backdrop p-5" onClick={(event) => { if (event.target === event.currentTarget) close(); }}>
    <div ref={dialogRef} role="dialog" aria-modal="true" aria-labelledby={titleId} tabIndex={-1}
      className="modal w-120 max-w-full max-h-[calc(100vh-2.5rem)] overflow-y-auto rounded-xl border border-border bg-background p-6 text-text shadow-modal">
      <div className="mb-5 flex items-center justify-between"><h2 id={titleId} className="m-0 text-xl font-medium">{m.sidebar_edit_project()}</h2><IconButton aria-label={m.tour_close()} onClick={close} disabled={pending}><X size={16} /></IconButton></div>
      <form onSubmit={(event) => { event.preventDefault(); void save(); }}>
        <label htmlFor={nameId} className="sr-only">{m.new_project_form_project_name()}</label>
        <div className="relative"><FolderOpen size={16} className="pointer-events-none absolute start-3 top-3 text-subtext" /><Input id={nameId} data-initial-focus value={name} onChange={(event) => setName(event.target.value)} disabled={pending} required className="h-10 ps-10" /></div>
        <div className="mt-5 mb-2 text-sm font-medium">{m.sidebar_source_folder()}</div>
        <div className="flex items-start gap-2 rounded-lg border border-border p-3 text-sm"><FolderOpen size={16} className="mt-0.5 shrink-0 text-subtext" /><span className="min-w-0 break-all">{project.repoPath}</span></div>
        {error && <p role="alert" className="mt-3 text-sm text-accent-red">{error}</p>}
        <div className="mt-6 flex items-center gap-2"><Button type="button" variant="danger" disabled={pending} onClick={() => void remove()}>{m.projects_home_delete_project_action()}</Button><div className="flex-1" /><Button type="button" variant="ghost" disabled={pending} onClick={close}>{m.projects_home_cancel()}</Button><Button type="submit" variant="primary" disabled={pending || !name.trim()}>{m.common_save()}</Button></div>
      </form>
    </div>
  </div>;
}
