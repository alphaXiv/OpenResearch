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
  getCodeTree,
  getSessionWorktree,
  githubBranchUrl,
  manageProjectFile,
  type CodeTree,
  type Project,
  type SessionWorktree,
} from "../api";
import { useSessionBusyRefresh } from "../events";
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
import { showAlert } from "./ui";

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
}: {
  sessionId?: string;
  project: Project;
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
  const [wt, setWt] = useState<SessionWorktree | null>(null);
  const [tree, setTree] = useState<CodeTree | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [contextMenu, setContextMenu] = useState<FileContextMenuTarget | null>(null);
  const [renamingPath, setRenamingPath] = useState<string | null>(null);
  // A request id drops stale responses — superseded refreshes, poll ticks, and
  // (via the effect-cleanup bump) post-unmount completions.
  const reqId = useRef(0);

  const load = useCallback(() => {
    const id = ++reqId.current;
    setLoading(true);
    const request = async (): Promise<[SessionWorktree | null, CodeTree]> => {
      if (!sessionId) {
        return [null, await getCodeTree(projectId, { ref: project.baselineBranch })];
      }
      const worktree = await getSessionWorktree(sessionId);
      const source = worktree.exists ? { sessionId } : { ref: project.baselineBranch };
      return [worktree, await getCodeTree(projectId, source)];
    };
    request()
      .then(([w, t]) => {
        if (id !== reqId.current) return;
        setWt(w);
        setTree(t);
        setError(null);
      })
      .catch((e: Error) => {
        if (id !== reqId.current) return;
        // Keep the last-good data — a transient git failure (index.lock while
        // the agent commits) shouldn't blank the view.
        setError(e.message);
      })
      .finally(() => {
        if (id === reqId.current) setLoading(false);
      });
  }, [sessionId, projectId, project.baselineBranch]);

  // Fetch on mount and whenever the bound session changes; the cleanup bump
  // invalidates in-flight responses on session change and unmount.
  useEffect(() => {
    setWt(null);
    setTree(null);
    setError(null);
    load();
    return () => {
      reqId.current++;
    };
  }, [load]);

  useSessionBusyRefresh(projectId, sessionId, true, load);

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
  const canManageFiles = tree?.root === "worktree" || (tree?.root === "clone" && !sessionId);
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
        refreshing={loading}
        onRefresh={load}
     />
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
            <CodeTabNote>{m.worktree_tab_no_files()}</CodeTabNote>
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
