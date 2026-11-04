import { useMutation, useQuery } from "@tanstack/react-query";
import { Copy, Download, Link2, X } from "lucide-react";
import { useRef, useState, type RefObject } from "react";

import { getProjectLink, publishProject, unpublishProject, type Project, type ShareChoice } from "../api";
import { showConfirmDialog, type DialogContentProps } from "../dialog";
import { m } from "../paraglide/messages.js";
import { listChatSessionsQuery } from "../queries/chat";
import { queryClient, workspaceKey } from "../queries/client";
import { openSignIn } from "./AccountChip";
import { Button, ButtonLink, DialogContent, DialogTitle, IconButton, Input, LoadingRow, Spinner, showAlert } from "./ui";

export function ShareProjectDialog({ project, remote, returnFocus, cancel, confirm, isDisabled: pending, isPending, errorMessage }: DialogContentProps<ShareChoice> & {
  project: Project;
  remote: boolean;
  returnFocus: RefObject<HTMLButtonElement | null>;
}) {
  const chatsQuery = useQuery(listChatSessionsQuery(project.id));
  const chats = chatsQuery.data?.filter((chat) => !chat.sideParentSessionId);
  const [unpicked, setUnpicked] = useState<string[]>([]);
  const picked = chats?.map((chat) => chat.id).filter((id) => !unpicked.includes(id)) ?? [];
  const [logs, setLogs] = useState(true);
  const [artifacts, setArtifacts] = useState(true);
  const choice: ShareChoice = { chats: picked, logs, artifacts };
  // Asked of the machine that publishes: in an SSH workspace that is the remote, not this laptop.
  const linkKey = workspaceKey("getProjectLink", project.id);
  const linkQuery = useQuery({
    queryKey: linkKey,
    queryFn: ({ signal }) => getProjectLink(project.id, signal),
    // `orx login` on the remote happens in a shell, so nothing else tells this dialog.
    refetchInterval: (query) => remote && query.state.data?.signedIn === false ? 3_000 : false,
  });
  const signedIn = linkQuery.data?.signedIn === true;
  const link = linkQuery.data?.sharedProject ?? null;
  const publish = useMutation({
    mutationFn: () => publishProject(project.id, choice),
    onSuccess: ({ sharedProject, skipped }) => {
      queryClient.setQueryData(linkKey, { signedIn: true, sharedProject });
      if (skipped.length > 0) showAlert(m.share_skipped({ paths: skipped.join(", ") }), "warning");
    },
  });
  const unpublish = useMutation({
    mutationFn: () => unpublishProject(project.id),
    onSuccess: () => queryClient.setQueryData(linkKey, { signedIn: true, sharedProject: null }),
  });
  const publishRef = useRef<HTMLButtonElement>(null);
  const busy = pending || publish.isPending || unpublish.isPending;
  const linkError = publish.error ?? unpublish.error;
  const ready = Boolean(chats) || Boolean(chatsQuery.error);

  return <DialogContent finalFocus={returnFocus} className="max-h-[calc(100vh-2.5rem)] overflow-y-auto text-text">
    <div className="mb-4 flex items-center justify-between">
      <DialogTitle>{m.share_dialog_title({ name: project.name })}</DialogTitle>
      <IconButton type="button" aria-label={m.tour_close()} onClick={cancel} disabled={busy}><X size={16} /></IconButton>
    </div>
    <p className="text-sm">{m.share_explainer()}</p>
    <div className="mt-4 flex flex-col gap-2 text-sm">
      <label className="flex items-center gap-2"><input type="checkbox" checked={logs} disabled={busy} onChange={(event) => setLogs(event.target.checked)} />{m.share_logs()}</label>
      <label className="flex items-center gap-2"><input type="checkbox" checked={artifacts} disabled={busy} onChange={(event) => setArtifacts(event.target.checked)} />{m.share_artifacts()}</label>
    </div>
    <fieldset className="mt-5 text-sm">
      <legend className="font-medium">{m.share_chats()}</legend>
      <p className="mt-1 text-xs text-subtext">{m.share_chats_hint()}</p>
      <div className="mt-2 max-h-48 overflow-y-auto rounded-lg border border-border p-2">
        {chatsQuery.error ? <p role="alert" className="px-1 text-accent-red">{chatsQuery.error.message}</p>
          : !chats ? <LoadingRow /> : chats.length === 0 ? <p className="px-1 text-subtext">{m.share_no_chats()}</p>
          : chats.map((chat) => <label key={chat.id} className="flex items-center gap-2 rounded-sm px-1 py-1 hover:bg-surface">
            <input type="checkbox" checked={picked.includes(chat.id)} disabled={busy}
              onChange={(event) => setUnpicked((cur) => event.target.checked ? cur.filter((id) => id !== chat.id) : [...cur, chat.id])} />
            <span className="truncate">{chat.title || m.share_untitled_chat()}</span>
          </label>)}
      </div>
    </fieldset>
    <section className="mt-5 text-sm">
      <p className="text-xs text-subtext">{remote && linkQuery.data && !signedIn ? m.share_publish_remote_sign_in() : m.share_publish_hint()}</p>
      {link && <div className="mt-2">
        <label className="flex flex-col gap-1">
          <span className="font-medium">{m.share_link_label()}</span>
          <Input readOnly value={link.url} onFocus={(event) => event.currentTarget.select()} />
        </label>
        <div className="mt-2 flex flex-wrap gap-2">
          <Button type="button" size="small" onClick={() => {
            void navigator.clipboard.writeText(link.url).then(() => showAlert(m.share_link_copied(), "success"));
          }}><Copy size={14} />{m.share_copy_link()}</Button>
          <ButtonLink size="small" target="_blank" rel="noreferrer"
            href={`https://x.com/intent/post?${new URLSearchParams({ text: m.share_post_text({ name: project.name }), url: link.url })}`}>{m.share_post_x()}</ButtonLink>
          <Button type="button" size="small" variant="ghost" disabled={busy} onClick={() => showConfirmDialog({
            title: m.share_unpublish_title(),
            description: m.share_unpublish_description(),
            confirmLabel: m.share_unpublish(),
            destructive: true,
            onConfirm: async () => { await unpublish.mutateAsync(); },
            onCancel: "close",
          })}>{unpublish.isPending && <Spinner />}{m.share_unpublish()}</Button>
        </div>
      </div>}
    </section>
    {(linkError || errorMessage) && <p role="alert" className="mt-3 whitespace-pre-line text-sm text-accent-red">{linkError?.message ?? errorMessage}</p>}
    <div className="mt-6 flex flex-wrap justify-end gap-2">
      <Button type="button" variant="ghost" disabled={busy} onClick={cancel}>{m.projects_home_cancel()}</Button>
      <Button ref={publishRef} type="button" disabled={busy || !ready || !linkQuery.data || (remote && !signedIn)} onClick={() => {
        if (signedIn) publish.mutate();
        else openSignIn(false, publishRef.current);
      }}>{publish.isPending ? <Spinner /> : <Link2 size={16} />}{linkQuery.data && !signedIn ? m.share_publish_sign_in() : link ? m.share_republish() : m.share_publish()}</Button>
      <Button type="button" variant="primary" disabled={busy || !ready} onClick={() => { publish.reset(); confirm(choice); }}>{isPending ? <Spinner /> : <Download size={16} />}{m.share_save()}</Button>
    </div>
  </DialogContent>;
}
