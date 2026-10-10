import { createContext, useContext, useState, type ContextType, type KeyboardEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import type { FileLocationRequest } from "../api";
import { revealFileInManager } from "../api";
import { loadedFileLocation } from "../fileLocation";
import { getFileLocationQuery, resolvedFileQuery } from "../queries/files";
import { isCurrentScope, workspaceScope } from "../queries/client";
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { showAlert } from "./ui";
import { FileContextMenu, copyAbsoluteFilePath, fileContextMenuTarget, type FileContextMenuEvent, type FileContextMenuTarget } from "./FileTreeActions";

export interface FileActionScopeValue {
  sessionId?: string;
  ref?: string;
  source?: "repo" | "artifacts" | "abs";
}
export const FileActionScope = createContext<FileActionScopeValue | null>(null);
export const FileActionsContext = createContext<{
  projectId: string;
  remote: boolean;
  resolve: (path: string, scope: FileActionScopeValue | null, exp?: string) => FileLocationRequest | null;
} | null>(null);

export function useFileLinkMenu(path: string, exp?: string) {
  const context = useContext(FileActionsContext);
  const scope = useContext(FileActionScope);
  const [target, setTarget] = useState<FileContextMenuTarget | null>(null);
  const open = (event: FileContextMenuEvent) => {
    if (!context) return;
    event.preventDefault();
    event.stopPropagation();
    setTarget(fileContextMenuTarget(event, path));
  };
  return {
    handlers: context ? {
      onContextMenu: open,
      onKeyDown: (event: KeyboardEvent<HTMLButtonElement>) => {
        if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) open(event);
      },
    } : {},
    menu: target && context ? { target, context, request: context.resolve(path, scope, exp), onClose: () => setTarget(null) } : null,
  };
}

export function FileLinkMenu({ target, context, request, onClose, onOpen }: {
  target: FileContextMenuTarget;
  context: NonNullable<ContextType<typeof FileActionsContext>>;
  request: FileLocationRequest | null;
  onClose: () => void;
  onOpen: () => void;
}) {
  const scope = workspaceScope();
  const file = useQuery({
    ...resolvedFileQuery(context.projectId, request?.path ?? "", request?.source ?? "repo", request?.sessionId, request?.ref),
    enabled: request !== null,
  });
  const loaded = file.data;
  const resolved = loaded ? loadedFileLocation(loaded, request?.sessionId, request?.ref) : request;
  const location = useQuery({
    ...getFileLocationQuery(context.projectId, resolved ?? { path: "", source: "repo" }),
    enabled: !!loaded && !loaded.file.notFound && !file.isFetching,
  });
  const path = !file.isFetching && !location.isFetching && !file.isError && !location.isError && !loaded?.file.notFound
    ? location.data?.absolutePath : null;
  const status = !request || loaded?.file.notFound ? m.file_viewer_not_found()
    : file.error?.message ?? location.error?.message
    ?? (path ? (context.remote ? m.file_actions_remote_path({ path: ltr(path) }) : ltr(path))
      : request.ref ? m.file_viewer_os_action_committed_version({ branch: ltr(request.ref) })
        : m.file_actions_resolving());
  return <FileContextMenu
    target={target}
    openLabel={m.file_actions_open_in_panel()}
    copyLabel={m.file_actions_copy_absolute_path()}
    onOpen={onOpen}
    pathStatus={status}
    onCopyPath={path ? () => { if (isCurrentScope(scope)) copyAbsoluteFilePath(path); } : undefined}
    onReveal={path && !context.remote && resolved ? () => {
      if (!isCurrentScope(scope)) return;
      void revealFileInManager(context.projectId, resolved.path, resolved)
        .catch(e => showAlert(e instanceof Error ? e.message : String(e), "error"));
    } : undefined}
    onClose={onClose}
  />;
}
