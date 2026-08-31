import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
// Files and committed changes for one experiment branch. The opening
// experiment fixes the Git source; users only switch between Files/Changes.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  getCodeTree,
  getSessionWorktree,
  githubBranchUrl,
  type CodeTree,
  type Experiment,
  type Project,
} from "../api";
import { BranchChanges } from "./BranchChanges";
import { CodeBrowserHeader, type CodeBrowserView } from "./CodeBrowserHeader";
import { buildTree, TreeLevel } from "./codeTree";
import type { TabOpenIntent } from "../tabPreview";
import { CodeTabBody, CodeTabNote } from "./layout/TabBody";

export type CodeView = CodeBrowserView;

export function CodeTab({
  projectId,
  project,
  experiment,
  view,
  toggled,
  onViewChange,
  onToggledChange,
  onOpenFile,
}: {
  projectId: string;
  /** Owning project — supplies owner/repo for the GitHub branch link. */
  project: Project;
  /** Experiment whose committed Git branch this tab displays. */
  experiment: Experiment;
  view: CodeView;
  /** Dirs flipped away from their depth default (lives on the tab def). */
  toggled: ReadonlySet<string>;
  onViewChange: (view: CodeView) => void;
  onToggledChange: (toggled: ReadonlySet<string>) => void;
  /** Open a file in the right pane's FileViewer, keyed to this source. */
  onOpenFile: (
    path: string,
    sessionId: string | undefined,
    ref: string | undefined,
    intent: TabOpenIntent,
  ) => void;
}) {
  const branch = experiment.branchName;
  const sourceKey = `${projectId}:${branch}`;
  const [data, setData] = useState<CodeTree | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [changesLoading, setChangesLoading] = useState(false);
  const [changesRefreshKey, setChangesRefreshKey] = useState(0);
  // The experiment's session worktree, when it exists and is still checked out
  // on this branch, is the on-disk copy to edit — so files open editable there
  // instead of as read-only committed blobs. Absent → committed view (read-only).
  const [editSessionId, setEditSessionId] = useState<string | undefined>(undefined);
  // A request id drops stale responses — from earlier sources, superseded
  // refreshes, and (via the effect-cleanup bump) post-unmount completions.
  const reqId = useRef(0);
  const requestedSource = useRef<string | null>(null);

  const load = useCallback(() => {
    requestedSource.current = sourceKey;
    const id = ++reqId.current;
    setLoading(true);
    getCodeTree(projectId, { ref: branch })
      .then((d) => {
        if (id !== reqId.current) return;
        setData(d);
        setError(null);
      })
      .catch((e: Error) => {
        if (id !== reqId.current) return;
        setError(e.message);
      })
      .finally(() => {
        if (id === reqId.current) setLoading(false);
      });
  }, [projectId, branch, sourceKey]);

  // Clear a previous branch's tree immediately and invalidate its requests.
  useEffect(() => {
    reqId.current++;
    requestedSource.current = null;
    setData(null);
    setError(null);
    setLoading(false);
    return () => {
      reqId.current++;
    };
  }, [sourceKey]);

  // Changes can open without paying for an unused tree request. Load the tree
  // once when Files is first shown; manual Refresh can still call load again.
  useEffect(() => {
    if (view === "files" && requestedSource.current !== sourceKey) load();
  }, [view, sourceKey, load]);

  // Resolve whether this branch is live on the creating session's worktree; only
  // then are its files on disk under that branch and safe to edit via the
  // session. Any other case (no session, pruned worktree, session moved to
  // another branch) leaves files read-only.
  useEffect(() => {
    setEditSessionId(undefined);
    const sid = experiment.chatSessionId;
    if (!sid) return;
    let cancelled = false;
    getSessionWorktree(sid)
      .then((wt) => {
        if (!cancelled && wt.exists && wt.branch === branch) setEditSessionId(sid);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [experiment.chatSessionId, branch]);

  const tree = useMemo(() => (data ? buildTree(data.entries) : null), [data]);
  const refreshing = view === "files" ? loading : changesLoading;

  const toggle = useCallback(
    (path: string) => {
      const next = new Set(toggled);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      onToggledChange(next);
    },
    [toggled, onToggledChange],
  );

  return (
    <div className="code-tab flex flex-col h-full min-h-0">
      <CodeBrowserHeader
        view={view}
        onViewChange={onViewChange}
        branchLabel={branch}
        branchTitle={`Committed branch ${branch}`}
        githubHref={
          project.githubEnabled
            ? githubBranchUrl(project.githubOwner, project.githubRepo, branch)
            : undefined
        }
        githubTitle={m.a11y_open_branch_github({ branch: ltr(branch) })}
        refreshing={refreshing}
        onRefresh={() =>
          view === "files" ? load() : setChangesRefreshKey((current) => current + 1)
        }
     />
      {view === "changes" ? (
        <BranchChanges
          key={experiment.id}
          experiment={experiment}
          refreshKey={changesRefreshKey}
          onLoadingChange={setChangesLoading}
       />
      ) : (
        <>
          {data?.truncated && <CodeTabNote>{m.code_tab_listing_truncated()}</CodeTabNote>}
          {error && tree && <CodeTabNote>{m.code_tab_refresh_failed()} {ltr(error)}</CodeTabNote>}
          <CodeTabBody>
            {!tree ? (
              <CodeTabNote>
                {error ? m.common_failed_to_load({ error: ltr(error) }) : m.common_loading()}
              </CodeTabNote>
            ) : tree.dirs.size === 0 && tree.files.length === 0 ? (
              <CodeTabNote>{m.code_tab_no_files()}</CodeTabNote>
            ) : (
              <div className="file-tree py-1.5 px-0 text-sm">
                <TreeLevel
                  node={tree}
                  parentPath=""
                  depth={0}
                  toggled={toggled}
                  onToggle={toggle}
                  onOpenFile={(path, intent) =>
                    editSessionId
                      ? onOpenFile(path, editSessionId, undefined, intent)
                      : onOpenFile(path, undefined, branch, intent)
                  }
               />
              </div>
            )}
          </CodeTabBody>
        </>
      )}
    </div>
  );
}
