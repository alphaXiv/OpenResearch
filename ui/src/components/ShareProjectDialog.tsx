import { useQuery } from "@tanstack/react-query";
import { Download, X } from "lucide-react";
import { useState, type RefObject } from "react";

import type { Project, ShareChoice } from "../api";
import type { DialogContentProps } from "../dialog";
import { m } from "../paraglide/messages.js";
import { useWorkspace } from "../workspace";
import { Button, DialogContent, DialogTitle, IconButton, LoadingRow, Spinner } from "./ui";

export function ShareProjectDialog({ project, returnFocus, cancel, confirm, isDisabled: pending, isPending, errorMessage }: DialogContentProps<ShareChoice> & {
  project: Project;
  returnFocus: RefObject<HTMLButtonElement | null>;
}) {
  const { listChatSessionsQuery } = useWorkspace().queries;
  const chatsQuery = useQuery(listChatSessionsQuery(project.id));
  const chats = chatsQuery.data?.filter((chat) => !chat.sideParentSessionId);
  const [unpicked, setUnpicked] = useState<string[]>([]);
  const picked = chats?.map((chat) => chat.id).filter((id) => !unpicked.includes(id)) ?? [];
  const [logs, setLogs] = useState(true);
  const [artifacts, setArtifacts] = useState(true);

  return <DialogContent finalFocus={returnFocus} className="max-h-[calc(100vh-2.5rem)] overflow-y-auto text-text">
    <div className="mb-4 flex items-center justify-between">
      <DialogTitle>{m.share_dialog_title({ name: project.name })}</DialogTitle>
      <IconButton type="button" aria-label={m.tour_close()} onClick={cancel} disabled={pending}><X size={16} /></IconButton>
    </div>
    <p className="text-sm">{m.share_explainer()}</p>
    <div className="mt-4 flex flex-col gap-2 text-sm">
      <label className="flex items-center gap-2"><input type="checkbox" checked={logs} disabled={pending} onChange={(event) => setLogs(event.target.checked)} />{m.share_logs()}</label>
      <label className="flex items-center gap-2"><input type="checkbox" checked={artifacts} disabled={pending} onChange={(event) => setArtifacts(event.target.checked)} />{m.share_artifacts()}</label>
    </div>
    <fieldset className="mt-5 text-sm">
      <legend className="font-medium">{m.share_chats()}</legend>
      <p className="mt-1 text-xs text-subtext">{m.share_chats_hint()}</p>
      <div className="mt-2 max-h-48 overflow-y-auto rounded-lg border border-border p-2">
        {chatsQuery.error ? <p role="alert" className="px-1 text-accent-red">{chatsQuery.error.message}</p>
          : !chats ? <LoadingRow /> : chats.length === 0 ? <p className="px-1 text-subtext">{m.share_no_chats()}</p>
          : chats.map((chat) => <label key={chat.id} className="flex items-center gap-2 rounded-sm px-1 py-1 hover:bg-surface">
            <input type="checkbox" checked={picked.includes(chat.id)} disabled={pending}
              onChange={(event) => setUnpicked((cur) => event.target.checked ? cur.filter((id) => id !== chat.id) : [...cur, chat.id])} />
            <span className="truncate">{chat.title || m.share_untitled_chat()}</span>
          </label>)}
      </div>
    </fieldset>
    {errorMessage && <p role="alert" className="mt-3 whitespace-pre-line text-sm text-accent-red">{errorMessage}</p>}
    <div className="mt-6 flex justify-end gap-2">
      <Button type="button" variant="ghost" disabled={pending} onClick={cancel}>{m.projects_home_cancel()}</Button>
      <Button type="button" variant="primary" disabled={pending || (!chats && !chatsQuery.error)} onClick={() => confirm({ chats: picked, logs, artifacts })}>{isPending ? <Spinner /> : <Download size={16} />}{m.share_save()}</Button>
    </div>
  </DialogContent>;
}
