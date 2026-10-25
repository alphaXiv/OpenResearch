import { useQuery } from "@tanstack/react-query";
import { Copy, FolderOpen } from "lucide-react";
import type { FileLocationRequest } from "../api";
import { revealFileInManager } from "../api";
import { getFileLocationQuery } from "../queries/files";
import { isCurrentScope, workspaceScope } from "../queries/client";
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { copyAbsoluteFilePath } from "./FileTreeActions";
import { IconButton, showAlert, Spinner } from "./ui";

export function FilePathDetails({ projectId, request, remote }: {
  projectId: string;
  request: FileLocationRequest;
  remote: boolean;
}) {
  const location = useQuery(getFileLocationQuery(projectId, request));
  const scope = workspaceScope();
  const path = !location.isError && !location.isFetching ? location.data?.absolutePath : null;
  return <div className="flex shrink-0 min-w-0 items-center gap-1 border-b border-border-variant px-3 py-1 text-xs text-subtext">
    <span dir="auto" className="min-w-0 flex-1 truncate" title={path ?? location.error?.message}>
      {location.error?.message ?? (path
        ? remote ? m.file_actions_remote_path({ path: ltr(path) }) : ltr(path)
        : request.ref ? m.file_viewer_os_action_committed_version({ branch: ltr(request.ref) }) : m.file_actions_resolving())}
    </span>
    {location.isPending && <Spinner />}
    {path && <>
      <IconButton size="small" aria-label={m.file_actions_copy_absolute_path()} data-tip={m.file_actions_copy_absolute_path()} onClick={() => { if (isCurrentScope(scope)) copyAbsoluteFilePath(path); }}><Copy size={13} /></IconButton>
      {!remote && <IconButton size="small" aria-label={m.file_viewer_reveal_in_file_manager()} data-tip={m.file_viewer_reveal_in_file_manager()} onClick={() => {
        if (!isCurrentScope(scope)) return;
        void revealFileInManager(projectId, request.path, request).catch(e => showAlert(e instanceof Error ? e.message : String(e), "error"));
      }}><FolderOpen size={13} /></IconButton>}
    </>}
  </div>;
}
