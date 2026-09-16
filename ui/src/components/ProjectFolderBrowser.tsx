import { useEffect, useRef, useState } from "react";
import { browseProjectFolder, type FolderListing } from "../api";
import { Button } from "./ui";
import { FolderOpen, HardDrive, ArrowUp, Home, ChevronRight } from "lucide-react";
import { m } from "../paraglide/messages.js";

/// Navigable path segments between the root crumb and the current folder.
/// `C:\a\b` → [C:\, a, b]; `/a/b` → [a, b] under the "/" root crumb.
function folderCrumbs(path: string): { label: string; path: string }[] {
  const parts = path.split(/[\\/]+/).filter(Boolean);
  const crumbs: { label: string; path: string }[] = [];
  if (/^[A-Za-z]:$/.test(parts[0] ?? "")) {
    let acc = parts[0] + "\\";
    crumbs.push({ label: acc, path: acc });
    for (const part of parts.slice(1)) {
      acc += part;
      crumbs.push({ label: part, path: acc });
      acc += "\\";
    }
  } else {
    let acc = "";
    for (const part of parts) {
      acc += "/" + part;
      crumbs.push({ label: part, path: acc });
    }
  }
  return crumbs;
}

export function ProjectFolderBrowser({ initialPath, label, onSelect, onCancel }: {
  initialPath: string;
  label?: string;
  onSelect: (path: string) => void;
  onCancel: () => void;
}) {
  const [listing, setListing] = useState<FolderListing | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const request = useRef<AbortController | null>(null);

  async function browse(path: string) {
    request.current?.abort();
    const controller = new AbortController();
    request.current = controller;
    let timedOut = false;
    const timeout = setTimeout(() => { timedOut = true; controller.abort(); }, 10_000);
    setBusy(true);
    setError(null);
    try {
      const result = await browseProjectFolder(path.trim(), controller.signal);
      if (request.current !== controller) return;
      setListing(result);
      setEditing(false);
    } catch (err) {
      if (request.current !== controller) return;
      // A failed typed path keeps the draft so it can be corrected.
      setError(timedOut ? m.folder_browser_timeout() : err instanceof Error ? err.message : String(err));
    } finally {
      clearTimeout(timeout);
      if (request.current === controller) setBusy(false);
    }
  }

  useEffect(() => {
    void browse(initialPath);
    return () => {
      request.current?.abort();
      request.current = null;
    };
  }, []);

  const currentPath = listing?.path ?? "";
  const windows = /^[A-Za-z]:[\\/]/.test(currentPath);
  const crumbs = folderCrumbs(currentPath);
  const startEdit = () => {
    setDraft(currentPath);
    setEditing(true);
  };

  return (
    <section className="flex flex-col gap-2 rounded-md border border-border p-3" aria-label={label ?? m.new_project_choose_existing_folder()}>
      <div className="flex items-center gap-2">
        <Button type="button" size="small" disabled={!listing?.path || busy}
          title={m.folder_browser_parent()} aria-label={m.folder_browser_parent()}
          onClick={() => void browse(listing?.parent ?? "")}><ArrowUp size={14} /></Button>
        {editing ? (
          <input autoFocus className="min-w-0 flex-1" value={draft} dir="ltr" spellCheck={false}
            placeholder={m.folder_browser_path_hint()} aria-label={m.folder_browser_path_hint()}
            onChange={(event) => setDraft(event.target.value)}
            onBlur={() => setEditing(false)}
            onKeyDown={(event) => {
              if (event.key === "Enter") { event.preventDefault(); void browse(draft); }
              if (event.key === "Escape") { event.preventDefault(); setEditing(false); }
            }} />
        ) : (
          <nav className="flex min-w-0 flex-1 cursor-text items-center gap-0.5 overflow-x-auto whitespace-nowrap rounded-md border border-border px-2 py-1 text-sm"
            aria-label={m.folder_browser_locations()} title={m.folder_browser_path_hint()}
            onClick={(event) => { if (event.target === event.currentTarget) startEdit(); }}>
            {currentPath ? (
              <button type="button" className="shrink-0 rounded-sm px-1 text-subtext hover:text-text"
                onClick={() => void browse(windows ? "" : "/")}>
                {windows ? m.folder_browser_computer() : "/"}
              </button>
            ) : (
              <button type="button" className="truncate rounded-sm px-1 font-medium text-text"
                onClick={startEdit}>{m.folder_browser_computer()}</button>
            )}
            {crumbs.map((crumb, index) => (
              <span key={crumb.path} className="flex min-w-0 items-center gap-0.5">
                <ChevronRight size={12} className="shrink-0 text-muted" />
                {index === crumbs.length - 1 ? (
                  <button type="button" className="truncate rounded-sm px-1 font-medium text-text"
                    onClick={startEdit}>{crumb.label}</button>
                ) : (
                  <button type="button" className="truncate rounded-sm px-1 text-subtext hover:text-text"
                    onClick={() => void browse(crumb.path)}>{crumb.label}</button>
                )}
              </span>
            ))}
          </nav>
        )}
      </div>
      {busy && <p role="status">{m.file_viewer_loading()}</p>}
      {error && <p className="break-all text-sm text-accent-red" role="alert">{error}</p>}
      {listing && <div className="flex max-h-60 flex-col overflow-y-auto" aria-label={m.folder_browser_directories()}>
        {!listing.path && <Button type="button" variant="ghost" className="!justify-start" title="~"
          onClick={() => void browse("~")}>
          <Home size={15} className="shrink-0" />
          <span className="truncate">{m.folder_browser_home()}</span>
        </Button>}
        {listing.folders.map((folder) => <Button type="button" variant="ghost" className="!justify-start" key={folder.path}
          onClick={() => void browse(folder.path)} title={folder.name}>
          {listing.path ? <FolderOpen size={15} className="shrink-0" /> : <HardDrive size={15} className="shrink-0" />}
          <span className="truncate" dir="auto">{folder.name}</span>
        </Button>)}
        {!listing.folders.length && <p>{m.folder_browser_empty()}</p>}
      </div>}
      <div className="flex justify-end gap-2 border-t border-border pt-2">
        <Button type="button" onClick={onCancel}>{m.new_project_form_cancel()}</Button>
        <Button type="button" variant="primary" disabled={busy || !listing?.path}
          onClick={() => listing?.path && onSelect(listing.path)}>{m.folder_browser_select()}</Button>
      </div>
    </section>
  );
}
