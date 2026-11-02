import { useQuery, useMutation } from "@tanstack/react-query";
import { setScopedQueryData, isCurrentScope } from "../queries/client";
import { getSshConfigQuery } from "../queries/settings";
import { X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { showConfirmDialog, showCustomDialog, type DialogContentProps } from "../dialog";
import { saveSshConfig, type SshConfigFile } from "../api";
import { m } from "../paraglide/messages.js";
import { CodeEditor } from "./CodeEditor";
import { Button, DialogContent, DialogTitle, IconButton, showAlert, Spinner } from "./ui";

export function showSshConfigDialog(onClose?: () => void) {
  const beforeCancel = { current: () => Promise.resolve() };

  showCustomDialog({
    component: SshConfigDialog,
    props: { beforeCancel },
    onConfirm: () => {},
    onCancel: async () => {
      await beforeCancel.current();
      onClose?.();
    },
  });
}

export function SshConfigDialog({
  cancel,
  isDisabled,
  beforeCancel,
  onSaved,
}: DialogContentProps<void> & {
  beforeCancel: { current: () => Promise<void> };
  onSaved?: () => void;
}) {
  const options = getSshConfigQuery();
  const fileQuery = useQuery(options);
  const [file, setFile] = useState<SshConfigFile | null>(() => fileQuery.data ?? null);
  const [draft, setDraft] = useState(() => fileQuery.data?.content ?? "");
  const loadError = !file && fileQuery.error ? fileQuery.error.message : null;
  const saveMutation = useMutation({ mutationFn: ({ content, previous }: { content: string; previous: string }) => saveSshConfig(content, previous) });
  const [saving, setSaving] = useState(false);
  const dialogRef = useRef<HTMLDivElement>(null);
  const dirty = file !== null && draft !== file.content;
  const dirtyRef = useRef(dirty);
  const savingRef = useRef(saving);
  dirtyRef.current = dirty;
  savingRef.current = saving;

  useEffect(() => {
    if (!fileQuery.data || dirtyRef.current || savingRef.current) return;
    setFile(fileQuery.data);
    setDraft(fileQuery.data.content);
  }, [fileQuery.data]);

  beforeCancel.current = () => {
    if (savingRef.current) return Promise.reject();
    if (!dirtyRef.current) return Promise.resolve();

    return new Promise<void>((resolve, reject) => {
      showConfirmDialog({
        title: m.ssh_config_title(),
        description: m.ssh_config_discard_changes(),
        destructive: true,
        onConfirm: resolve,
        onCancel: () => { reject(); },
      });
    });
  };

  async function save() {
    if (!file || !dirty || saving || isDisabled) return;
    setSaving(true);
    try {
      await saveMutation.mutateAsync({ content: draft, previous: file.content });
      if (!isCurrentScope(options.queryKey)) return;
      setScopedQueryData(options.queryKey, { ...file, content: draft });
      setFile({ ...file, content: draft });
      onSaved?.();
      showAlert(m.ssh_config_saved(), "success");
    } catch (error) {
      showAlert(error instanceof Error ? error.message : String(error), "error");
    } finally {
      setSaving(false);
    }
  }

  return <DialogContent
      ref={dialogRef}
      initialFocus={() => dialogRef.current?.querySelector("textarea") ?? true}
      className="relative flex h-[min(48rem,calc(100vh-2.5rem))] w-200 max-w-full flex-col overflow-hidden p-0"
      aria-labelledby="ssh-config-dialog-title"
    >
      <div className="shrink-0 px-6 pt-5 pb-4 pe-14">
        <DialogTitle id="ssh-config-dialog-title">
          {m.ssh_config_title()}
        </DialogTitle>
        <code className="mt-1 block font-mono text-sm text-subtext">~/.ssh/config</code>
      </div>
      <IconButton
        className="absolute end-3.5 top-3.5"
        aria-label={m.ssh_config_close()}
        onClick={cancel}
        disabled={saving || isDisabled}
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
            readOnly={isDisabled}
            path={file.path}
          />
        )}
      </div>
      <div className="flex shrink-0 justify-end gap-2.5 p-4">
        <Button onClick={cancel} disabled={saving || isDisabled}>{m.settings_page_cancel()}</Button>
        <Button variant="primary" onClick={() => void save()} disabled={!dirty || saving || isDisabled}>
          {saving ? m.common_saving() : m.common_save()}
        </Button>
      </div>
    </DialogContent>;
}
