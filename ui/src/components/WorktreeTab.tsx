import { FolderOpen, FolderSync } from "lucide-react";
import { WorkspaceEmptyState } from "./WorkspaceEmptyState";
import { isImmutableQuery } from "../queries/invalidation";
import { useQuery } from "@tanstack/react-query";
import { listChatSessionsQuery } from "../queries/chat";
import {
  workspaceKey,
  queryClient,
} from "../queries/client";

import { getCodeTreeQuery, getSessionStorageQuery, getSessionWorktreeQuery } from "../queries/files";
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
// The pinned Files home for the active chat session's private worktree — what
// the agent is changing right now, before any run/commit exists. The Code tab
// remains committed-state only.
//
//   Files (default): the full live worktree tree.
//   Changes: the unified diff vs the baseline merge-base, untracked
//     files included as new-file chunks — the same per-file-card rendering as
//     the experiment Changes view (the header's file count comes from a
//     separate git pass, so it stays truthful even when the diff truncates).
//
// Freshness without idle churn: poll every 5 s only while the session is busy
// (chat.busy SSE), refresh once on the busy→idle edge, and a manual refresh
// button always works. Transient errors (an index.lock race while the agent
// commits) keep the last-good data with a small "refresh failed" note, mirroring
// CodeTab's staleness handling.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  fmtNumber,
  githubBranchUrl,
  manageProjectFile,
  revealFileInManager,
  revealSessionStorage,
  syncSessionWorktree,
  type Project,
  type SessionStoragePaths,
} from "../api";

import { CodeBrowserHeader, type CodeBrowserView } from "./CodeBrowserHeader";
import { buildTree, TreeLevel } from "./codeTree";
import { GitDiffExplorer, TruncatedDiffNotice } from "./GitDiff";
import type { TabOpenIntent } from "../tabPreview";
import { CodeTabBody, CodeTabNote } from "./layout/TabBody";
import {
  FileContextMenu,
  copyFilePath,
  fileContextMenuTarget,
  type FileContextMenuTarget,
} from "./FileTreeActions";
import { IconButton, showAlert, Spinner } from "./ui";

export type WorktreeView = CodeBrowserView;

export function WorktreeTab({
  sessionId,
  project,
  view,
  toggled,
  onViewChange,
  onToggledChange,
  onOpenFile,
  canRenameFile,
  remote,
}: {
  sessionId?: string;
  project: Project;
  /** True when `orx up` serves this dashboard over SSH — OS file actions
   * (reveal in file manager) act on the remote box, so they're local-only. */
  remote?: boolean;
  /** Which segmented view is showing (lives on the tab def, so it survives the
   * unmount/remount when another end-pane tab fronts this one). */
  view: WorktreeView;
  /** Files-view dirs flipped away from their depth default (on the tab def). */
  toggled: ReadonlySet<string>;
  onViewChange: (view: WorktreeView) => void;
  onToggledChange: (toggled: ReadonlySet<string>) => void;
  /** Open a file in the right pane's FileViewer, keyed to this worktree. */
  onOpenFile: (
    path: string,
    sessionId: string | undefined,
    ref: string | undefined,
    intent: TabOpenIntent,
  ) => void;
  canRenameFile: (path: string) => boolean;
}) {
  const projectId = project.id;
  const sessions = useQuery(listChatSessionsQuery(projectId));
  const busy = sessions.data?.some((session) => session.id === sessionId && session.busy) ?? false;
  const worktree = useQuery({ ...getSessionWorktreeQuery(sessionId ?? ""), enabled: Boolean(sessionId), refetchInterval: busy ? 5_000 : false });
  const wt = worktree.data;
  const source = sessionId && wt?.exists ? { sessionId } : { ref: project.baselineBranch };
  const files = useQuery({ ...getCodeTreeQuery(projectId, source), enabled: !sessionId || worktree.isSuccess, refetchInterval: busy ? 5_000 : false });
  const tree = files.data;
  const error = (worktree.error ?? files.error)?.message;
  const loading = worktree.isFetching || files.isFetching;
  const [contextMenu, setContextMenu] = useState<FileContextMenuTarget | null>(null);
  const [renamingPath, setRenamingPath] = useState<string | null>(null);
  const load = useCallback(() => {
    if (sessionId) {
      void queryClient.invalidateQueries(getSessionWorktreeQuery(sessionId));
      void queryClient.invalidateQueries(getSessionStorageQuery(sessionId));
    }
    void queryClient.invalidateQueries({ queryKey: workspaceKey("getCodeTree", projectId), predicate: (query) => !isImmutableQuery(query) });
  }, [sessionId, projectId]);
  const wasBusy = useRef(busy);
  useEffect(() => {
    if (wasBusy.current && !busy) load();
    wasBusy.current = busy;
  }, [busy, load]);

  const filesTree = useMemo(() => (tree ? buildTree(tree.entries) : null), [tree]);

  const toggle = useCallback(
    (path: string) => {
      const next = new Set(toggled);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      onToggledChange(next);
    },
    [toggled, onToggledChange],
  );

  const liveWorktree = sessionId && wt?.exists ? wt : null;
  const checkedOut =
    liveWorktree?.branch ??
    (liveWorktree?.baselineBranch ? m.worktree_detached_at({ branch: ltr(liveWorktree.baselineBranch) }) : m.settings_detached());
  const fileCount = liveWorktree?.files?.length ?? 0;
  const branchChip = liveWorktree
    ? m.worktree_current({ branch: ltr(`${checkedOut}${fileCount > 0 ? "*" : ""}`) })
    : m.worktree_default_branch({ branch: ltr(project.baselineBranch) });
  const githubBranch = liveWorktree ? liveWorktree.branch : project.baselineBranch;
  const openFile = (path: string, intent: TabOpenIntent) =>
    liveWorktree
      ? onOpenFile(path, sessionId, undefined, intent)
      : onOpenFile(path, undefined, project.baselineBranch, intent);
  const canManageFiles = tree?.root === "worktree";
  const manage = async (path: string, action: Parameters<typeof manageProjectFile>[2]) => {
    try {
      await manageProjectFile(projectId, path, action, {
        sessionId,
      });
      load();
    } catch (error) {
      showAlert(error instanceof Error ? error.message : String(error), "error");
    }
  };
  const copyPath = (path: string) => {
    const root = tree?.path ?? project.repoPath;
    copyFilePath(root, path);
  };
  const [syncing, setSyncing] = useState(false);
  const [showStorage, setShowStorage] = useState(false);
  const storage = useQuery({
    ...getSessionStorageQuery(sessionId ?? ""),
    enabled: Boolean(sessionId) && showStorage,
    refetchInterval: busy ? 5_000 : false,
  });
  /** Overlay this worktree's changes into the project folder — the explicit
   * "land my work in the folder I picked" action. */
  const sync = async () => {
    if (!sessionId) return;
    setSyncing(true);
    try {
      const result = await syncSessionWorktree(sessionId);
      const changes = result.copied + result.deleted;
      showAlert(
        changes === 0
          ? m.worktree_sync_none()
          : changes === 1
            ? m.worktree_sync_one()
            : m.worktree_sync_many({ count: fmtNumber(changes) }),
        changes === 0 ? "info" : "success",
      );
      load();
    } catch (error) {
      showAlert(error instanceof Error ? error.message : String(error), "error");
    } finally {
      setSyncing(false);
    }
  };
  const revealWorktree = async () => {
    if (!sessionId) return;
    try {
      await revealFileInManager(projectId, ".", { sessionId });
    } catch (error) {
      showAlert(error instanceof Error ? error.message : String(error), "error");
    }
  };
  const revealStorage = async (location: keyof SessionStoragePaths) => {
    if (!sessionId) return;
    try {
      await revealSessionStorage(sessionId, location);
    } catch (error) {
      showAlert(error instanceof Error ? error.message : String(error), "error");
    }
  };

  return (
    <div className="code-tab flex flex-col h-full min-h-0 wt-tab">
      <CodeBrowserHeader
        view={liveWorktree ? view : "files"}
        onViewChange={onViewChange}
        showViewToggle={Boolean(liveWorktree)}
        branchLabel={branchChip}
        branchTitle={branchChip}
        githubHref={
          project.githubEnabled && githubBranch
            ? githubBranchUrl(project.githubOwner, project.githubRepo, githubBranch)
            : undefined
        }
        githubTitle={githubBranch ? m.a11y_open_branch_github({ branch: ltr(githubBranch) }) : undefined}
        actions={liveWorktree && sessionId ? (
          <>
            <IconButton
              size="small"
              data-tip={remote ? m.file_viewer_os_action_local_only() : m.file_viewer_reveal_in_file_manager()}
              aria-label={m.file_viewer_reveal_in_file_manager()}
              disabled={remote}
              onClick={() => void revealWorktree()}
            >
              <FolderOpen size={13} />
            </IconButton>
            <IconButton
              size="small"
              data-tip={m.worktree_sync_to_folder()}
              aria-label={m.worktree_sync_to_folder()}
              disabled={syncing}
              onClick={() => void sync()}
            >
              {syncing ? <Spinner /> : <FolderSync size={13} />}
            </IconButton>
          </>
        ) : undefined}
        refreshing={loading}
        onRefresh={load}
      />
      {liveWorktree && sessionId && <CodeTabNote>{m.worktree_sync_note()}</CodeTabNote>}
      {sessionId && (
        <details
          className="shrink-0 border-b border-border-variant px-3 py-2 text-sm"
          onToggle={(event) => setShowStorage(event.currentTarget.open)}
        >
          <summary className="cursor-pointer text-subtext">{m.worktree_storage_locations()}</summary>
          <p className="my-2 text-muted">{m.worktree_storage_note()}</p>
          {storage.error && <p className="text-muted">{m.common_failed_to_load({ error: ltr(storage.error.message) })}</p>}
          {!storage.data && !storage.error && <Spinner />}
          {storage.data && ([
            ["records", m.worktree_storage_records()],
            ["transcript", m.worktree_storage_transcript()],
            ["attachments", m.worktree_storage_attachments()],
            ["artifacts", m.worktree_storage_artifacts()],
          ] as const).map(([location, label]) => (
            <div key={location} className="flex items-center gap-2 py-1">
              <div className="min-w-0 flex-1">
                <div>{label}</div>
                <code dir="ltr" className="block wrap-anywhere text-xs text-muted">{storage.data[location]}</code>
              </div>
              <IconButton
                size="small"
                aria-label={`${m.file_viewer_reveal_in_file_manager()}: ${label}`}
                data-tip={remote ? m.file_viewer_os_action_local_only() : m.file_viewer_reveal_in_file_manager()}
                disabled={remote}
                onClick={() => void revealStorage(location)}
              >
                <FolderOpen size={13} />
              </IconButton>
            </div>
          ))}
        </details>
      )}
      {error && (wt || tree) && <CodeTabNote>{m.worktree_tab_refresh_failed()} {ltr(error)}</CodeTabNote>}
      {!tree || (sessionId && !wt) ? (
        <CodeTabBody>
          <CodeTabNote>{error ? m.common_failed_to_load({ error: ltr(error) }) : m.common_loading()}</CodeTabNote>
        </CodeTabBody>
      ) : liveWorktree && view === "changes" ? (
        <CodeTabBody className="wt-changes px-4 pb-6 pt-0 [&_>_:first-child]:mt-3.5">
          {fileCount === 0 || !liveWorktree.diff ? (
            <div className="changes-note text-sm text-muted">{m.worktree_tab_no_changes_yet()}</div>
          ) : (
            <>
              {liveWorktree.diff.truncated && (
                <TruncatedDiffNotice
                  bytesRead={liveWorktree.diff.bytesRead}
                  byteLimit={liveWorktree.diff.byteLimit}
                />
              )}
              <GitDiffExplorer
                diff={liveWorktree.diff.diff}
                partial={liveWorktree.diff.truncated}
              />
            </>
          )}
        </CodeTabBody>
      ) : (
        <CodeTabBody>
          {tree.truncated && (
            <CodeTabNote>{m.worktree_tab_listing_truncated()}</CodeTabNote>
          )}
          {!filesTree ? (
            <CodeTabNote>{m.worktree_tab_loading()}</CodeTabNote>
          ) : filesTree.dirs.size === 0 && filesTree.files.length === 0 ? (
            <WorkspaceEmptyState
              icon={FolderOpen}
              title={m.worktree_tab_no_files()}
              description={m.files_empty_description()}
            />
          ) : (
            <div className="file-tree py-1.5 px-0 text-sm">
              <TreeLevel
                node={filesTree}
                parentPath=""
                depth={0}
                toggled={toggled}
                onToggle={toggle}
                onOpenFile={openFile}
                renamingPath={renamingPath}
                onContextMenu={(event, path) => {
                  setContextMenu(fileContextMenuTarget(event, path));
                }}
                onRename={(path, name) => {
                  setRenamingPath(null);
                  void manage(path, { action: "rename", newName: name });
                }}
                onCancelRename={() => setRenamingPath(null)}
              />
            </div>
          )}
        </CodeTabBody>
      )}
      {contextMenu && (
        <FileContextMenu
          target={contextMenu}
          onOpen={() => openFile(contextMenu.path, "keepOpen")}
          onRename={canManageFiles && canRenameFile(contextMenu.path)
            ? () => setRenamingPath(contextMenu.path)
            : undefined}
          onDuplicate={canManageFiles
            ? () => void manage(contextMenu.path, { action: "duplicate" })
            : undefined}
          onCopyPath={() => copyPath(contextMenu.path)}
          onDelete={canManageFiles
            ? () => {
              if (window.confirm(m.file_tree_delete_confirm({ path: ltr(contextMenu.path) })))
                void manage(contextMenu.path, { action: "delete" });
            }
            : undefined}
          onClose={() => setContextMenu(null)}
        />
      )}
    </div>
  );
}
