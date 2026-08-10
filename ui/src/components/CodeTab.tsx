// Files and committed changes for one experiment branch. The opening
// experiment fixes the Git source; users only switch between Files/Changes.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  getCodeTree,
  githubBranchUrl,
  type CodeTree,
  type Experiment,
  type Project,
} from "../api";
import { BranchChanges } from "./BranchChanges";
import { CodeBrowserHeader, type CodeBrowserView } from "./CodeBrowserHeader";
import { buildTree, TreeLevel } from "./codeTree";
import { CODE_TAB_BODY_CLASS_NAME, CODE_TAB_NOTE_CLASS_NAME } from "../styleClasses";

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
  onOpenFile: (path: string, sessionId?: string, ref?: string) => void;
}) {
  const branch = experiment.branchName;
  const sourceKey = `${projectId}:${branch}`;
  const [data, setData] = useState<CodeTree | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [changesLoading, setChangesLoading] = useState(false);
  const [changesRefreshKey, setChangesRefreshKey] = useState(0);
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
    <div className="code-tab [display:flex] [flex-direction:column] [height:100%] [min-height:0]">
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
        githubTitle={`Open ${branch} on GitHub`}
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
          {data?.truncated && <div className={CODE_TAB_NOTE_CLASS_NAME}>listing truncated</div>}
          {error && tree && <div className={CODE_TAB_NOTE_CLASS_NAME}>Refresh failed: {error}</div>}
          <div className={CODE_TAB_BODY_CLASS_NAME}>
            {!tree ? (
              <div className={CODE_TAB_NOTE_CLASS_NAME}>
                {error ? `Failed to load: ${error}` : "Loading…"}
              </div>
            ) : tree.dirs.size === 0 && tree.files.length === 0 ? (
              <div className={CODE_TAB_NOTE_CLASS_NAME}>No files.</div>
            ) : (
              <div className="file-tree [padding:6px_0] [font-size:var(--fs-md)]">
                <TreeLevel
                  node={tree}
                  parentPath=""
                  depth={0}
                  toggled={toggled}
                  onToggle={toggle}
                  onOpenFile={(path) => onOpenFile(path, undefined, branch)}
                />
              </div>
            )}
          </div>
        </>
      )}
    </div>
  );
}
