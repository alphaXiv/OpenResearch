import { useMutate } from "@clo/react-mutation";
import { useQuery } from "@tanstack/react-query";
import { Copy, Download, Link2, X } from "lucide-react";
import { useRef, useState, type RefObject } from "react";

import type { OperationProgress, Project, ShareChoice } from "../api";
import type { DialogContentProps } from "../dialog";
import { m } from "../paraglide/messages.js";
import { useWorkspace } from "../workspace";
import { mutPublishProject } from "../queries/projects";
import { useSignIn } from "./AccountChip";
import { OperationProgressBar } from "./OperationProgressBar";
import { Button, ButtonLink, DialogContent, DialogTitle, IconButton, Input, Spinner, showAlert } from "./ui";

export function showShareExclusions(count: number) {
  if (count === 0) return;
  showAlert(count === 1 ? m.share_skipped_one() : m.share_skipped({ count }), "warning");
}

export function ShareProjectDialog({ project, remote, returnFocus, cancel, confirm, isDisabled: pending, isPending, errorMessage }: DialogContentProps<ShareChoice> & {
  project: Project;
  remote: boolean;
  returnFocus: RefObject<HTMLButtonElement | null>;
}) {
  const workspace = useWorkspace();
  const { listChatSessionsQuery, getProjectLinkQuery } = workspace.queries;
  const openSignIn = useSignIn();
  const chatsQuery = useQuery(listChatSessionsQuery(project.id));
  const chats = chatsQuery.data?.filter((chat) => !chat.sideParentSessionId);
  const choice: ShareChoice = { chats: chats?.map((chat) => chat.id) ?? [], logs: true, artifacts: true };
  // Asked of the machine that publishes: in an SSH workspace that is the remote, not this laptop.
  const linkQuery = useQuery({
    ...getProjectLinkQuery(project.id),
    // `orx login` on the remote happens in a shell, so nothing else tells this dialog.
    refetchInterval: (query) => remote && query.state.data?.signedIn === false ? 3_000 : false,
  });
  const signedIn = linkQuery.data?.signedIn === true;
  const link = linkQuery.data?.sharedProject ?? null;
  const publish = useMutate(mutPublishProject);
  const [progress, setProgress] = useState<OperationProgress | null>(null);
  const publishError = publish.error === undefined ? null : publish.error instanceof Error ? publish.error.message : String(publish.error);
  const publishRef = useRef<HTMLButtonElement>(null);
  const busy = pending || publish.isMutating;
  // Without the chat list the file would quietly leave every chat out, so sharing waits for it.
  const ready = Boolean(chats);
  const save = () => { publish.clear(); confirm(choice); };

  return <DialogContent finalFocus={returnFocus} className="relative max-h-[calc(100vh-2.5rem)] overflow-y-auto text-text">
    <IconButton type="button" className="absolute end-3.5 top-3.5" aria-label={m.tour_close()} onClick={cancel} disabled={busy}><X size={16} /></IconButton>
    <DialogTitle className="mb-3 pe-8 break-words">{m.share_dialog_title({ name: project.name })}</DialogTitle>
    {!link && <p className="text-sm">{m.share_explainer()}</p>}
    {chatsQuery.error && <p role="alert" className="mt-3 break-words text-sm text-accent-red">{chatsQuery.error.message}</p>}
    <section className="mt-2 text-sm">
      {!link ? <p className="text-xs text-subtext">
        {linkQuery.error ? m.share_publish_unavailable() : remote && linkQuery.data && !signedIn ? m.share_publish_remote_sign_in() : m.share_publish_hint()}
      </p> : <div>
        <label className="flex flex-col gap-1">
          <span className="font-medium">{m.share_link_label()}</span>
          <Input readOnly value={link.url} onFocus={(event) => event.currentTarget.select()} />
        </label>
        <div className="mt-2 flex flex-wrap gap-2">
          <Button type="button" size="small" onClick={() => {
            void navigator.clipboard.writeText(link.url).then(() => showAlert(m.share_link_copied(), "success"));
          }}><Copy size={14} />{m.share_copy_link()}</Button>
          <ButtonLink size="small" target="_blank" rel="noreferrer"
            className="border-text bg-text text-background no-underline [&:hover:not(:disabled)]:bg-text/85 [&:active:not(:disabled)]:bg-text/75"
            href={`https://x.com/intent/post?${new URLSearchParams({ text: m.share_post_text({ name: project.name }), url: link.url })}`}>
            <svg aria-hidden viewBox="0 0 24 24" className="size-3.5 fill-current"><path d="M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-5.214-6.817L4.99 21.75H1.68l7.73-8.835L1.254 2.25H8.08l4.713 6.231zm-1.161 17.52h1.833L7.084 4.126H5.117z" /></svg>
            {m.share_post_x()}
          </ButtonLink>
          <Button type="button" size="small" variant="primary" className="ms-auto" disabled={busy || !ready} onClick={save}>{isPending ? <Spinner /> : <Download size={14} />}{m.share_save()}</Button>
        </div>
      </div>}
    </section>
    {publish.isMutating && <OperationProgressBar progress={progress} />}
    {(publishError || errorMessage) && <p role="alert" className="mt-3 whitespace-pre-line break-words text-sm text-accent-red">{publishError ?? errorMessage}</p>}
    {!link && <div className="mt-4 flex flex-wrap justify-end gap-2">
      <Button ref={publishRef} type="button" disabled={busy || !ready || !linkQuery.data || (remote && !signedIn)} onClick={() => {
        if (!signedIn) {
          openSignIn(false, publishRef.current);
          return;
        }
        setProgress(null);
        publish.runWithOptions(workspace, project.id, choice, setProgress, {
          onSuccessData: ({ skipped }) => showShareExclusions(skipped.length),
        });
      }}>{publish.isPending ? <Spinner /> : <Link2 size={16} />}{linkQuery.data && !signedIn ? m.share_publish_sign_in() : m.share_publish()}</Button>
      <Button type="button" variant="primary" disabled={busy || !ready} onClick={save}>{isPending ? <Spinner /> : <Download size={16} />}{m.share_save()}</Button>
    </div>}
  </DialogContent>;
}
