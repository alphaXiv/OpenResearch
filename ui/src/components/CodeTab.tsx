import { useIsFetching, useQuery } from "@tanstack/react-query";

import { queryClient } from "../queries/client";
import { getCodeTreeQuery, getSessionWorktreeQuery, getExperimentDiffQuery } from "../queries/files";
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
// Files and committed changes for one experiment branch. The opening
// experiment fixes the Git source; users only switch between Files/Changes.

import { useCallback, useMemo } from "react";
import {
  githubBranchUrl,
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
  const files = useQuery({ ...getCodeTreeQuery(projectId, { ref: branch }), enabled: view === "files", subscribed: view === "files" });
  const worktree = useQuery({ ...getSessionWorktreeQuery(experiment.chatSessionId ?? ""), enabled: view === "files" && Boolean(experiment.chatSessionId), subscribed: view === "files" && Boolean(experiment.chatSessionId) });
  const data = files.data;
  const error = files.error?.message;
  const loading = files.isFetching;
  const diffOptions = getExperimentDiffQuery(experiment.id);
  const changesLoading = useIsFetching({ queryKey: diffOptions.queryKey }) > 0;
  const editSessionId = worktree.data?.exists && worktree.data.branch === branch ? experiment.chatSessionId ?? undefined : undefined;
  const load = () => { void files.refetch(); };

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
          view === "files" ? load() : void queryClient.invalidateQueries(diffOptions)
        }
      />
      {view === "changes" ? (
        <BranchChanges
          key={experiment.id}
          experiment={experiment}
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
