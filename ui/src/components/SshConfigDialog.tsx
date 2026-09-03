import { X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { getSshConfig, saveSshConfig, type SshConfigFile } from "../api";
import { m } from "../paraglide/messages.js";
import { CodeEditor } from "./CodeEditor";
import { Button, IconButton, showAlert, Spinner } from "./ui";
import { useDialogFocus } from "./useDialogFocus";

export function SshConfigDialog({
  onClose,
  onSaved,
}: {
  onClose: () => void;
  onSaved?: () => void;
}) {
  const [file, setFile] = useState<SshConfigFile | null>(null);
  const [draft, setDraft] = useState("");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const dialogRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  const dirty = file !== null && draft !== file.content;
  const dirtyRef = useRef(dirty);
  const savingRef = useRef(saving);
  dirtyRef.current = dirty;
  savingRef.current = saving;
  onCloseRef.current = onClose;

  useEffect(() => {
    getSshConfig()
      .then((loaded) => {
        setFile(loaded);
        setDraft(loaded.content);
      })
      .catch((error) => setLoadError(error instanceof Error ? error.message : String(error)));
  }, []);

  const close = () => {
    if (savingRef.current) return;
    if (dirtyRef.current && !window.confirm(m.ssh_config_discard_changes())) return;
    onCloseRef.current();
  };

  useDialogFocus(dialogRef, close, "textarea");

  async function save() {
    if (!file || !dirty || saving) return;
    setSaving(true);
    try {
      await saveSshConfig(draft, file.content);
      setFile({ ...file, content: draft });
      onSaved?.();
      showAlert(m.ssh_config_saved(), "success");
    } catch (error) {
      showAlert(error instanceof Error ? error.message : String(error), "error");
    } finally {
      setSaving(false);
    }
  }

  return createPortal(
    <div
      className="fixed inset-0 z-200 flex items-center justify-center bg-modal-backdrop p-5"
      onClick={(event) => {
        if (event.target === event.currentTarget) close();
      }}
    >
      <div
        ref={dialogRef}
        className="relative flex h-[min(48rem,calc(100vh-2.5rem))] w-200 max-w-full flex-col overflow-hidden rounded-xl border border-border bg-background shadow-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="ssh-config-dialog-title"
        tabIndex={-1}
      >
        <div className="shrink-0 px-6 pt-5 pb-4 pe-14">
          <h2 id="ssh-config-dialog-title" className="m-0 text-xl font-medium">
            {m.ssh_config_title()}
          </h2>
          <code className="mt-1 block font-mono text-sm text-subtext">~/.ssh/config</code>
        </div>
        <IconButton
          className="absolute end-3.5 top-3.5"
          aria-label={m.remote_dialog_close()}
          onClick={close}
          disabled={saving}
        >
          <X size={16} />
        </IconButton>
        <div className="file-view min-h-0 flex-1 border-y border-border-variant bg-background">
          {loadError ? (
            <p className="m-5 text-sm text-accent-red">{loadError}</p>
          ) : file === null ? (
            <div className="flex items-center gap-2 p-5 text-sm text-subtext">
              <Spinner /> {m.ssh_config_loading()}
            </div>
          ) : (
            <CodeEditor
              value={draft}
              onChange={setDraft}
              onSave={() => void save()}
              path={file.path}
            />
          )}
        </div>
        <div className="flex shrink-0 justify-end gap-2.5 p-4">
          <Button onClick={close} disabled={saving}>{m.settings_page_cancel()}</Button>
          <Button variant="primary" onClick={() => void save()} disabled={!dirty || saving}>
            {saving ? m.common_saving() : m.common_save()}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
