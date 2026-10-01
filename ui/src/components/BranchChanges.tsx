import { useQuery } from "@tanstack/react-query";
import { getExperimentDiffQuery } from "../queries/files";
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { type Experiment } from "../api";
import { GitDiffExplorer, TruncatedDiffNotice } from "./GitDiff";
import { CodeTabBody, CodeTabNote } from "./layout/TabBody";

export function BranchChanges({
  experiment,
}: {
  experiment: Experiment;
}) {
  const query = useQuery(getExperimentDiffQuery(experiment.id));
  const diff = query.data;
  const error = query.error?.message;

  return (
    <CodeTabBody className="branch-changes [&_>_.changes-note]:mx-4 [&_>_.changes-note]:my-3.5 [&_>_.diff-explorer]:mx-4 [&_>_.diff-explorer]:mb-0 [&_>_.diff-explorer]:mt-3.5 [&_>_.openresearch-diff]:mx-4 [&_>_.openresearch-diff]:mb-0 [&_>_.openresearch-diff]:mt-3.5 [&_>_.truncated-notice]:mx-4 [&_>_.truncated-notice]:mb-0 [&_>_.truncated-notice]:mt-3.5">
      {error && !diff ? (
        <CodeTabNote>{m.branch_changes_failed_to_load_changes()} {ltr(error)}</CodeTabNote>
      ) : !diff ? (
        <CodeTabNote>{m.branch_changes_loading_changes()}</CodeTabNote>
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
    </CodeTabBody>
  );
}
