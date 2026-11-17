import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { parseDiff, type FileData } from "react-diff-view";
import { fmtNumber } from "../api";
import { ltr } from "../i18n";
import { m } from "../paraglide/messages.js";
import { useWorkspace } from "../workspace";
import { countChanges } from "./GitDiff";

export function RunDiffStat({ runId, parentSlug }: { runId: string | null; parentSlug: string | null }) {
  const { getRunDiffQuery } = useWorkspace().queries;
  const { data: diff, error } = useQuery({ ...getRunDiffQuery(runId ?? ""), enabled: Boolean(runId), subscribed: Boolean(runId) });
  const diffStat = useMemo(() => {
    if (!diff) return null;
    let text = diff.diff;
    if (diff.truncated) {
      // The backend byte-caps mid-line, which can crash the parser (a cut
      // inside an @@ header) — drop the trailing partial file so the
      // counts stay honest lower bounds.
      const cut = text.lastIndexOf("\ndiff --git ");
      text = cut !== -1 ? text.slice(0, cut + 1) : text.slice(0, text.lastIndexOf("\n") + 1);
    }
    let files: FileData[] = [];
    try {
      files = text.trim() ? parseDiff(text) : [];
    } catch {
      return null;
    }
    // A truncated single-file diff can trim down to a bare header that
    // parses as one file with no hunks; "≥ +0 −0 · 1+ files" is noise.
    if (diff.truncated && files.every((f) => f.hunks.length === 0)) return null;
    let additions = 0;
    let deletions = 0;
    for (const f of files) {
      const c = countChanges(f);
      additions += c.additions;
      deletions += c.deletions;
    }
    return { fileCount: files.length, additions, deletions, truncated: diff.truncated };
  }, [diff]);

  if (!runId) return null;
  if (error) return <span>{error.message}</span>;
  if (diff && !diffStat) return <span>{m.git_diff_parse_failed()}</span>;
  if (!diffStat || diffStat.fileCount === 0) return null;
  return (
    <span title={diffStat.truncated
      ? m.a11y_committed_changes_truncated({ parent: ltr(parentSlug ?? "parent") })
      : m.a11y_committed_changes({ parent: ltr(parentSlug ?? "parent") })}>
      {diffStat.truncated && "≥ "}
      <span className="diff-stat-add text-accent-green">+{fmtNumber(diffStat.additions)}</span>{" "}
      <span className="diff-stat-del text-accent-red">−{fmtNumber(diffStat.deletions)}</span>
      {" · "}
      {diffStat.fileCount === 1 && !diffStat.truncated
        ? m.hover_one_file()
        : diffStat.truncated ? m.hover_files_at_least({ count: fmtNumber(diffStat.fileCount) }) : m.hover_file_count({ count: fmtNumber(diffStat.fileCount) })}
    </span>
  );
}
