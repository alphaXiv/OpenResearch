import { useEffect, useRef, useState } from "react";
import { browseProjectFolder, type FolderListing } from "../api";
import { Button } from "./ui";
import { FolderOpen, HardDrive, ArrowUp, Home } from "lucide-react";
import { m } from "../paraglide/messages.js";

export function ProjectFolderBrowser({ initialPath, label, onSelect, onCancel }: {
  initialPath: string;
  label?: string;
  onSelect: (path: string) => void;
  onCancel: () => void;
}) {
  const [input, setInput] = useState(initialPath);
  const [listing, setListing] = useState<FolderListing | null>(null);
  const [roots, setRoots] = useState<FolderListing["roots"]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const request = useRef<AbortController | null>(null);

  async function browse(path: string) {
    request.current?.abort();
    const controller = new AbortController();
    request.current = controller;
    const timeout = setTimeout(() => controller.abort(), 10_000);
    setBusy(true);
    setListing(null);
    setError(null);
    try {
      const result = await browseProjectFolder(path.trim(), controller.signal);
      if (request.current !== controller) return;
      setListing(result);
      setInput(result.path ?? "");
      setRoots(result.roots);
    } catch (err) {
      if (request.current !== controller) return;
      setError(controller.signal.aborted ? m.folder_browser_timeout() : err instanceof Error ? err.message : String(err));
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

  return (
    <section className="flex flex-col gap-2 rounded-md border border-border p-3" aria-label={label ?? m.new_project_choose_existing_folder()}>
      <nav className="flex flex-wrap items-center gap-2" aria-label={m.folder_browser_locations()}>
        <Button type="button" size="small" onClick={() => void browse("")}><HardDrive size={14} />{m.folder_browser_computer()}</Button>
        <Button type="button" size="small" onClick={() => void browse("~")}><Home size={14} />{m.folder_browser_home()}</Button>
        {roots.map((root) => <Button key={root.path} type="button" size="small" onClick={() => void browse(root.path)}>{root.name}</Button>)}
      </nav>
      <label>
        <span>{m.new_project_form_project_location()}</span>
        <input value={input} dir="ltr" spellCheck={false} placeholder={m.folder_browser_path_hint()}
          onChange={(event) => setInput(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") { event.preventDefault(); void browse(input); }
          }} />
      </label>
      <div className="flex flex-wrap gap-2">
        <Button type="button" onClick={() => void browse(input)}>{m.folder_browser_open()}</Button>
        <Button type="button" disabled={!listing?.path || busy}
          onClick={() => void browse(listing?.parent ?? "")}><ArrowUp size={14} />{m.folder_browser_parent()}</Button>
      </div>
      {busy && <p role="status">{m.file_viewer_loading()}</p>}
      {error && <p className="break-all text-sm text-accent-red" role="alert">{error}</p>}
      {listing && <div className="flex max-h-60 flex-col overflow-y-auto" aria-label={m.folder_browser_directories()}>
        {listing.folders.map((folder) => <Button type="button" variant="ghost" className="!justify-start" key={folder.path}
          onClick={() => void browse(folder.path)} title={folder.name}>
          {listing.path ? <FolderOpen size={15} className="shrink-0" /> : <HardDrive size={15} className="shrink-0" />}
          <span className="truncate" dir="auto">{folder.name}</span>
        </Button>)}
        {!listing.folders.length && <p>{m.folder_browser_empty()}</p>}
      </div>}
      <div className="flex flex-wrap gap-2">
        <Button type="button" variant="primary" disabled={busy || !listing?.path || input !== listing.path}
          onClick={() => listing?.path && onSelect(listing.path)}>{m.folder_browser_select()}</Button>
        <Button type="button" onClick={onCancel}>{m.new_project_form_cancel()}</Button>
      </div>
    </section>
  );
}
