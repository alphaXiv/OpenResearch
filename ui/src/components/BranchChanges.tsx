import { m } from "../paraglide/messages.js";
import { useEffect, useState } from "react";
import { getExperimentDiff, type DiffPayload, type Experiment } from "../api";
import { GitDiffExplorer, TruncatedDiffNotice } from "./GitDiff";
import { CODE_TAB_BODY_CLASS_NAME, CODE_TAB_NOTE_CLASS_NAME } from "../styleClasses";

export function BranchChanges({
  experiment,
  refreshKey,
  onLoadingChange,
}: {
  experiment: Experiment;
  refreshKey: number;
  onLoadingChange: (loading: boolean) => void;
}) {
  const [diff, setDiff] = useState<DiffPayload | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    onLoadingChange(true);
    setError(null);
    setDiff(null);
    getExperimentDiff(experiment.id)
      .then((payload) => {
        if (!cancelled) setDiff(payload);
      })
      .catch((cause: Error) => {
        if (!cancelled) setError(cause.message);
      })
      .finally(() => {
        if (!cancelled) onLoadingChange(false);
      });
    return () => {
      cancelled = true;
    };
  }, [experiment.id, refreshKey, onLoadingChange]);

  return (
    <div className={`${CODE_TAB_BODY_CLASS_NAME} branch-changes [&_>_.changes-note]:my-3.5 [&_>_.changes-note]:mx-4 [&_>_.openresearch-diff]:mt-3.5 [&_>_.openresearch-diff]:mx-4 [&_>_.openresearch-diff]:mb-0 [&_>_.truncated-notice]:mt-3.5 [&_>_.truncated-notice]:mx-4 [&_>_.truncated-notice]:mb-0 [&_>_.diff-explorer]:mt-3.5 [&_>_.diff-explorer]:mx-4 [&_>_.diff-explorer]:mb-0`}>
      {error ? (
        <div className={CODE_TAB_NOTE_CLASS_NAME}>{m.branch_changes_failed_to_load_changes()} {error}</div>
      ) : !diff ? (
        <div className={CODE_TAB_NOTE_CLASS_NAME}>{m.branch_changes_loading_changes()}</div>
      ) : !diff.diff.trim() ? (
        <div className="changes-note text-sm text-muted">
          {experiment.parentExperimentId
            ? m.branch_no_committed_changes()
            : m.branch_baseline_no_parent()}
        </div>
      ) : (
        <>
          {diff.truncated && (
            <TruncatedDiffNotice bytesRead={diff.bytesRead} byteLimit={diff.byteLimit} />
          )}
          <GitDiffExplorer diff={diff.diff} partial={diff.truncated} />
        </>
      )}
    </div>
  );
}
