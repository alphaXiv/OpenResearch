import { useQuery } from "@tanstack/react-query";
import { useBlocker } from "@tanstack/react-router";
import { X } from "lucide-react";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { createPortal } from "react-dom";
import { saveAgentInstructions, type AgentInstructionsFile } from "../api";
import { m } from "../paraglide/messages.js";
import { getWorkspaceGeneration, isCurrentScope, setScopedQueryData, subscribeWorkspace } from "../queries/client";
import { getAgentInstructionsQuery } from "../queries/settings";
import { CodeEditor } from "./CodeEditor";
import { Button, IconButton, showAlert, Spinner } from "./ui";
import { useDialogFocus } from "./useDialogFocus";

export function AgentInstructionsSettings({ cardClassName }: { cardClassName: string }) {
  const [open, setOpen] = useState(false);
  const generation = useSyncExternalStore(subscribeWorkspace, getWorkspaceGeneration);
  return (
    <>
      <h2>{m.settings_instructions_title()}</h2>
      <div className={cardClassName}>
        <p className="mt-0 mb-3 text-sm leading-relaxed text-subtext">{m.settings_instructions_description()}</p>
        <Button onClick={() => setOpen(true)}>{m.settings_instructions_edit()}</Button>
      </div>
      {open && <AgentInstructionsDialog key={generation} onClose={() => setOpen(false)} />}
    </>
  );
}

export function AgentInstructionsDialog({ onClose }: { onClose: () => void }) {
  const options = getAgentInstructionsQuery();
  const fileQuery = useQuery(options);
  const [file, setFile] = useState<AgentInstructionsFile | null>(() => fileQuery.data ?? null);
  const [draft, setDraft] = useState(() => fileQuery.data?.content ?? "");
  const [busy, setBusy] = useState<"saving" | "loading" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const dirty = file !== null && draft !== file.content;
  const dirtyRef = useRef(dirty);
  const busyRef = useRef(false);
  dirtyRef.current = dirty;

  useEffect(() => {
    if (!fileQuery.data || dirtyRef.current || busyRef.current) return;
    setFile(fileQuery.data);
    setDraft(fileQuery.data.content);
  }, [fileQuery.data]);

  useBlocker({
    shouldBlockFn: () => busyRef.current
      || (dirtyRef.current && !window.confirm(m.settings_instructions_discard())),
    enableBeforeUnload: () => dirtyRef.current || busyRef.current,
  });

  const close = () => {
    if (busyRef.current) return;
    if (dirtyRef.current && !window.confirm(m.settings_instructions_discard())) return;
    onClose();
  };
  useDialogFocus(dialogRef, close, "textarea");

  // The focus trap mounts before the first fetch finishes. Focus the editor
  // when it appears, without stealing focus on subsequent file refreshes.
  const loaded = file !== null;
  useEffect(() => {
    if (loaded) dialogRef.current?.querySelector<HTMLTextAreaElement>("textarea")?.focus();
  }, [loaded]);

  async function save() {
    if (!file || !dirty || busyRef.current || !isCurrentScope(options.queryKey)) return;
    busyRef.current = true;
    setBusy("saving");
    setError(null);
    try {
      const saved = await saveAgentInstructions(draft, file.revision);
      if (!isCurrentScope(options.queryKey)) return;
      setScopedQueryData(options.queryKey, saved);
      setFile(saved);
      setDraft(saved.content);
      showAlert(m.settings_instructions_saved(), "success");
    } catch (error) {
      if (isCurrentScope(options.queryKey)) setError(error instanceof Error ? error.message : String(error));
    } finally {
      busyRef.current = false;
      setBusy(null);
    }
  }

  async function reload() {
    if (busyRef.current || !isCurrentScope(options.queryKey)) return;
    if (dirtyRef.current && !window.confirm(m.settings_instructions_discard())) return;
    busyRef.current = true;
    setBusy("loading");
    setError(null);
    try {
      const result = await fileQuery.refetch();
      if (!isCurrentScope(options.queryKey)) return;
      if (result.error) throw result.error;
      if (result.data) {
        setFile(result.data);
        setDraft(result.data.content);
      }
    } catch (error) {
      if (isCurrentScope(options.queryKey)) setError(error instanceof Error ? error.message : String(error));
    } finally {
      busyRef.current = false;
      setBusy(null);
    }
  }

  const message = error ?? fileQuery.error?.message;
  return createPortal(
    <div className="fixed inset-0 z-200 flex items-center justify-center bg-modal-backdrop p-5" onClick={(event) => {
      if (event.target === event.currentTarget) close();
    }}>
      <div ref={dialogRef} className="relative flex h-[min(48rem,calc(100vh-2.5rem))] w-200 max-w-full flex-col overflow-hidden rounded-xl border border-border bg-background shadow-modal"
        role="dialog" aria-modal="true" aria-labelledby="agent-instructions-title" aria-describedby="agent-instructions-description" tabIndex={-1}>
        <div className="shrink-0 px-6 pt-5 pb-4 pe-14">
          <h2 id="agent-instructions-title" className="m-0 text-xl font-medium">{m.settings_instructions_title()}</h2>
          {file && <code dir="ltr" className="mt-1 block break-all font-mono text-sm text-subtext">{file.path}</code>}
          <p id="agent-instructions-description" className="mt-2 mb-0 text-sm leading-relaxed text-subtext">{m.settings_instructions_description()}</p>
        </div>
        <IconButton className="absolute end-3.5 top-3.5" aria-label={m.window_close()} onClick={close} disabled={busy !== null}>
          <X size={16} />
        </IconButton>
        {message && <p role="alert" className="mx-6 mt-0 mb-3 text-sm text-accent-red whitespace-pre-wrap">{message}</p>}
        <div className="file-view min-h-0 flex-1 border-y border-border-variant bg-background" aria-busy={busy !== null || fileQuery.isPending}>
          {file ? <CodeEditor value={draft} onChange={setDraft} onSave={() => void save()} path={file.path}
            readOnly={busy !== null} ariaLabel={m.settings_instructions_title()} /> : !message && (
            <div className="flex items-center gap-2 p-5 text-sm text-subtext"><Spinner /> {m.common_loading()}</div>
          )}
        </div>
        <div className="flex shrink-0 flex-wrap items-center justify-between gap-2.5 p-4">
          <Button onClick={() => void reload()} disabled={busy !== null || fileQuery.isFetching}>
            {busy === "loading" ? m.common_loading() : m.settings_instructions_reload()}
          </Button>
          <div className="flex gap-2.5">
            <Button onClick={close} disabled={busy !== null}>{m.settings_page_cancel()}</Button>
            <Button variant="primary" onClick={() => void save()} disabled={!dirty || busy !== null}>
              {busy === "saving" ? m.common_saving() : m.common_save()}
            </Button>
          </div>
        </div>
      </div>
    </div>, document.body,
  );
}
