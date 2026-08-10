// Mirror of openresearch.sh's GitDiff: per-file collapsible cards over
// react-diff-view's unified view, with refractor syntax highlighting.

import { ChevronDown, ChevronRight } from "lucide-react";
import { useMemo, useState } from "react";
import {
  Diff,
  type ChangeData,
  type FileData,
  type HunkTokens,
  type RenderGutter,
  markEdits,
  parseDiff,
  tokenize,
} from "react-diff-view";
import { refractor } from "refractor";
import { detectSyntaxLanguageFromFilePath } from "../syntaxLanguage";
const DIFF_CLASS_NAME = [
  "openresearch-diff [display:flex] [flex-direction:column] [gap:16px]",
  "[&_.openresearch-diff-file]:[--openresearch-diff-selection-background-color:color-mix(_in_oklab,_var(--surface)_76%,_var(--primary)_)]",
  "[&_.openresearch-diff-file]:[--openresearch-diff-gutter-selection-background-color:color-mix(_in_oklab,_var(--surface)_68%,_var(--primary)_)]",
  "[&_.openresearch-diff-file]:[--openresearch-diff-insert-gutter-background-color:color-mix(_in_oklab,_var(--base)_84%,_var(--accent-green)_)]",
  "[&_.openresearch-diff-file]:[--openresearch-diff-delete-gutter-background-color:color-mix(_in_oklab,_var(--base)_86%,_var(--accent-red)_)]",
  "[&_.openresearch-diff-file]:[--openresearch-diff-insert-code-background-color:color-mix(_in_oklab,_var(--base)_91%,_var(--accent-green)_)]",
  "[&_.openresearch-diff-file]:[--openresearch-diff-delete-code-background-color:color-mix(_in_oklab,_var(--base)_92%,_var(--accent-red)_)]",
  "[&_.openresearch-diff-file]:[--openresearch-diff-insert-edit-background-color:color-mix(_in_oklab,_var(--base)_72%,_var(--accent-green)_)]",
  "[&_.openresearch-diff-file]:[--openresearch-diff-delete-edit-background-color:color-mix(_in_oklab,_var(--base)_78%,_var(--accent-red)_)]",
  "[&_.openresearch-diff-file]:[--openresearch-diff-divider-color:var(--border)]",
  "[&_.openresearch-diff-file]:[--openresearch-diff-omit-gutter-line-color:color-mix(in_oklab,_var(--base)_86%,_var(--text))]",
  "[&_.openresearch-diff-file]:[--openresearch-diff-unified-gutter-text-color:color-mix(in_oklab,_var(--text)_45%,_var(--base))]",
  "[&_.openresearch-diff-file]:[--diff-background-color:var(--base)]",
  "[&_.openresearch-diff-file]:[--diff-text-color:var(--text)]",
  "[&_.openresearch-diff-file]:[--diff-font-family:var(--mono)]",
  "[&_.openresearch-diff-file]:[--diff-selection-text-color:var(--primary)]",
  "[&_.openresearch-diff-file]:[--diff-selection-background-color:var(--openresearch-diff-selection-background-color)]",
  "[&_.openresearch-diff-file]:[--diff-gutter-selected-text-color:var(--diff-selection-text-color)]",
  "[&_.openresearch-diff-file]:[--diff-gutter-selected-background-color:var(--openresearch-diff-gutter-selection-background-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-selected-text-color:var(--diff-selection-text-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-selected-background-color:var(--diff-selection-background-color)]",
  "[&_.openresearch-diff-file]:[--diff-gutter-insert-text-color:var(--accent-green)]",
  "[&_.openresearch-diff-file]:[--diff-gutter-insert-background-color:var(--openresearch-diff-insert-gutter-background-color)]",
  "[&_.openresearch-diff-file]:[--diff-gutter-delete-text-color:var(--accent-red)]",
  "[&_.openresearch-diff-file]:[--diff-gutter-delete-background-color:var(--openresearch-diff-delete-gutter-background-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-insert-text-color:var(--diff-text-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-insert-background-color:var(--openresearch-diff-insert-code-background-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-delete-text-color:var(--diff-text-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-delete-background-color:var(--openresearch-diff-delete-code-background-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-insert-edit-text-color:var(--diff-text-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-insert-edit-background-color:var(--openresearch-diff-insert-edit-background-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-delete-edit-text-color:var(--diff-text-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-delete-edit-background-color:var(--openresearch-diff-delete-edit-background-color)]",
  "[&_.openresearch-diff-file]:[--diff-omit-gutter-line-color:var(--openresearch-diff-omit-gutter-line-color)]",
  "[&_.openresearch-diff-file]:[width:100%] [&_.openresearch-diff-file]:[font-size:var(--fs-sm)]",
  "[&_.openresearch-diff-file]:[line-height:1.55] [&_.openresearch-diff-file.diff-unified]:[table-layout:auto]",
  "[&_.openresearch-diff-file.diff-unified_col.diff-gutter-col:first-child]:[visibility:collapse]",
  "[&_.openresearch-diff-file.diff-unified_col.diff-gutter-col:first-child]:[width:0]",
  "[&_.openresearch-diff-file.diff-unified_col.diff-gutter-col:nth-child(2)]:[width:1%]",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:first-child]:[display:none]",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:[position:sticky]",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:[left:0]",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:[z-index:1]",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:[width:1%]",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:[padding:0_10px_0_14px]",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:[white-space:nowrap]",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:[text-align:right]",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:[color:var(--openresearch-diff-unified-gutter-text-color)]",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:[border-right:1px_solid_var(--openresearch-diff-divider-color)]",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:[user-select:none]",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:[cursor:default]",
  "[&_.openresearch-diff-file_.diff-line]:[line-height:1.55]",
  "[&_.openresearch-diff-file_.diff-line:has(.diff-code-insert)]:[background:var(--openresearch-diff-insert-code-background-color)]",
  "[&_.openresearch-diff-file_.diff-line:has(.diff-code-delete)]:[background:var(--openresearch-diff-delete-code-background-color)]",
  "[&_.openresearch-diff-file_.diff-code]:[padding:0_16px]",
  "[&_.openresearch-diff-file_.diff-code]:[white-space:pre]",
  "[&_.openresearch-diff-file_.diff-code]:[word-break:normal]",
  "[&_.openresearch-diff-file_.diff-code]:[overflow-wrap:normal]",
  "[&_.openresearch-diff-file_.diff-hunk_+_.diff-hunk_.diff-line:first-child_>_td]:[border-top:1px_solid_var(--openresearch-diff-divider-color)]",
].join(" ");

const HIGHLIGHT_MAX = 2000; // above this many changed lines, skip tokenizing

const REACT_DIFF_VIEW_REFRACTOR = {
  highlight(code: string, language: string) {
    return refractor.highlight(code, language).children;
  },
};

function getUnifiedLineNumber(change: ChangeData): number {
  if (change.type === "normal") return change.newLineNumber;
  return change.lineNumber;
}

export function countChanges(file: FileData) {
  let additions = 0;
  let deletions = 0;
  for (const hunk of file.hunks) {
    for (const change of hunk.changes) {
      if (change.type === "insert") additions++;
      else if (change.type === "delete") deletions++;
    }
  }
  return { additions, deletions };
}

function getHighlightPath(file: FileData): string | null {
  if (file.newPath === "/dev/null") return file.oldPath;
  if (file.oldPath === "/dev/null") return file.newPath;
  return file.newPath;
}

function formatDiffFilePath(file: FileData): string {
  switch (file.type) {
    case "delete":
      return file.oldPath;
    case "add":
    case "modify":
      return file.newPath;
    case "rename":
    case "copy":
      return `${file.oldPath} → ${file.newPath}`;
  }
}

function tokenizeDiffFile(file: FileData): HunkTokens {
  const enhancers = [markEdits(file.hunks, { type: "line" })];
  const language = detectSyntaxLanguageFromFilePath(getHighlightPath(file));
  if (language && refractor.registered(language)) {
    return tokenize(file.hunks, {
      enhancers,
      highlight: true,
      language,
      refractor: REACT_DIFF_VIEW_REFRACTOR,
    });
  }
  return tokenize(file.hunks, { enhancers, highlight: false });
}

function parseDiffFiles(diff: string, partial: boolean): { files: FileData[]; failed: boolean } {
  if (!diff.trim()) return { files: [], failed: false };
  try {
    return { files: parseDiff(diff, { nearbySequences: "zip" }), failed: false };
  } catch {
    if (partial) {
      const starts = Array.from(diff.matchAll(/^diff --git /gm), (match) => match.index);
      const lastStart = starts[starts.length - 1];
      if (starts.length > 1 && lastStart !== undefined) {
        try {
          return {
            files: parseDiff(diff.slice(0, lastStart), { nearbySequences: "zip" }),
            failed: false,
          };
        } catch {
          return { files: [], failed: true };
        }
      }
    }
    return { files: [], failed: true };
  }
}

const renderUnifiedGutter: RenderGutter = ({ change, side }) => {
  if (side === "old") return null;
  return getUnifiedLineNumber(change);
};

function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / 1024).toFixed(1)} KB`;
}

export function TruncatedDiffNotice({
  bytesRead,
  byteLimit,
}: {
  bytesRead: number;
  byteLimit: number;
}) {
  return (
    <div className="truncated-notice [border:1px_solid_var(--accent-amber)] [border-radius:var(--radius-md)] [background:var(--accent-amber-subtle)] [padding:12px_14px] [font-size:var(--fs-md)] [&_h4]:[margin:0_0_4px] [&_h4]:[font-size:var(--fs-md)] [&_h4]:[color:var(--accent-amber)] [&_p]:[margin:0] [&_p]:[color:var(--subtext)]">
      <h4>Diff preview truncated</h4>
      <p>
        Showing the first {formatBytes(byteLimit)} ({formatBytes(bytesRead)} read). View the complete
        diff locally with git.
      </p>
    </div>
  );
}

function DiffFileCard({
  file,
  defaultExpanded,
}: {
  file: FileData;
  defaultExpanded: boolean;
}) {
  const [expanded, setExpanded] = useState(defaultExpanded);
  const { additions, deletions } = useMemo(() => countChanges(file), [file]);
  const shouldTokenize = expanded && additions + deletions <= HIGHLIGHT_MAX;
  const tokens = useMemo<HunkTokens | undefined>(() => {
    if (!shouldTokenize) return undefined;
    try {
      return tokenizeDiffFile(file);
    } catch {
      return undefined; // tokenizing is best-effort
    }
  }, [file, shouldTokenize]);

  return (
    <section className={`diff-file-card [overflow:hidden] [border:1px_solid_var(--border)] [border-radius:var(--radius-md)] [background:var(--base)] [&.expanded_.diff-file-header]:[border-bottom:1px_solid_var(--border)] ${expanded ? "expanded" : ""}`}>
      <button
        className="diff-file-header [position:sticky] [top:0] [z-index:10] [display:flex] [align-items:center] [justify-content:space-between] [gap:12px] [width:100%] [text-align:left] [padding:8px_12px] [background:var(--canvas)] [cursor:pointer] [&_.chev]:[color:var(--muted)] [&_.chev]:[font-size:var(--fs-2xs)] [&_.chev]:[flex-shrink:0] [&_.chev]:[width:12px] [&_.path]:[display:flex] [&_.path]:[align-items:center] [&_.path]:[gap:8px] [&_.path]:[min-width:0] [&_.path]:[flex:1] [&_.path_code]:[min-width:0] [&_.path_code]:[flex:1] [&_.path_code]:[overflow:hidden] [&_.path_code]:[text-overflow:ellipsis] [&_.path_code]:[white-space:nowrap] [&_.path_code]:[font-family:var(--mono)] [&_.path_code]:[font-size:var(--fs-xs)] [&_.path_code]:[font-weight:var(--fw-semibold)] [&_.path_code]:[color:var(--text)] [&_.stats]:[display:flex] [&_.stats]:[align-items:center] [&_.stats]:[gap:8px] [&_.stats]:[flex-shrink:0] [&_.stats]:[font-family:var(--mono)] [&_.stats]:[font-size:var(--fs-2xs)] [&_.stats]:[font-weight:var(--fw-medium)] [&_.stats]:[font-variant-numeric:tabular-nums]"
        aria-expanded={expanded}
        onClick={() => setExpanded((e) => !e)}
      >
        <span className="chev">
          {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </span>
        <span className="path">
          <code>{formatDiffFilePath(file)}</code>
        </span>
        <span className="stats">
          <span className="diff-stat-add [color:var(--accent-green)]">+{additions}</span>
          <span className="diff-stat-del [color:var(--accent-red)]">−{deletions}</span>
        </span>
      </button>
      {expanded &&
        (file.hunks.length === 0 ? (
          <div className="diff-empty [padding:8px_12px] [color:var(--muted)] [font-size:var(--fs-md)]">No textual diff for this file.</div>
        ) : (
          <div className="diff-file-body [overflow-x:auto] [background:var(--base)]">
            <Diff
              className="openresearch-diff-file"
              diffType={file.type}
              gutterType="default"
              hunks={file.hunks}
              renderGutter={renderUnifiedGutter}
              tokens={tokens}
              viewType="unified"
            />
          </div>
        ))}
    </section>
  );
}

function DiffFiles({ files, className }: { files: FileData[]; className?: string }) {
  return (
    <div className={className ? `${DIFF_CLASS_NAME} ${className}` : DIFF_CLASS_NAME}>
      {files.map((file, i) => (
        <DiffFileCard
          key={`${file.oldPath}→${file.newPath}#${i}`}
          file={file}
          defaultExpanded={i === 0}
        />
      ))}
    </div>
  );
}

export function GitDiff({ diff, className }: { diff: string; className?: string }) {
  const parsed = useMemo(() => parseDiffFiles(diff, false), [diff]);
  if (parsed.failed) return <div className="diff-empty [padding:8px_12px] [color:var(--muted)] [font-size:var(--fs-md)]">Unable to parse this diff.</div>;
  if (parsed.files.length === 0) return <div className="diff-empty [padding:8px_12px] [color:var(--muted)] [font-size:var(--fs-md)]">No changes.</div>;
  return <DiffFiles files={parsed.files} className={className} />;
}

function fileStatus(file: FileData): string {
  switch (file.type) {
    case "add":
      return "A";
    case "delete":
      return "D";
    case "rename":
      return "R";
    case "copy":
      return "C";
    case "modify":
      return "M";
  }
}

export function GitDiffExplorer({ diff, partial = false }: { diff: string; partial?: boolean }) {
  const parsed = useMemo(() => parseDiffFiles(diff, partial), [diff, partial]);
  const files = parsed.files;
  const items = useMemo(
    () =>
      files.map((file, index) => ({
        file,
        key: `${file.oldPath}→${file.newPath}#${index}`,
        changes: countChanges(file),
      })),
    [files],
  );
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [showFullDiff, setShowFullDiff] = useState(false);
  const showingFullDiff = showFullDiff && !partial;
  const activeKey = items.some((item) => item.key === selectedKey)
    ? selectedKey
    : (items[0]?.key ?? null);
  const selected = items.find((item) => item.key === activeKey) ?? null;

  if (parsed.failed) {
    return (
      <div className="diff-empty [padding:8px_12px] [color:var(--muted)] [font-size:var(--fs-md)]">
        {partial ? "No complete file preview was available before the cutoff." : "Unable to parse this diff."}
      </div>
    );
  }
  if (items.length === 0) return <div className="diff-empty [padding:8px_12px] [color:var(--muted)] [font-size:var(--fs-md)]">No changes.</div>;

  return (
    <div className="diff-explorer [container-type:inline-size]">
      <div className="diff-explorer-toolbar [display:flex] [align-items:center] [justify-content:space-between] [gap:12px] [margin-bottom:10px] [font-size:var(--fs-sm)] [&_button]:[padding:2px_0] [&_button]:[color:var(--muted)] [&_button]:[font-size:var(--fs-xs)] [&_button]:[font-weight:var(--fw-medium)] [&_button:hover]:[color:var(--text)] [&_button:hover]:[text-decoration:underline] [&_button:hover]:[text-underline-offset:2px]">
        <strong>
          {partial
            ? `${items.length} ${items.length === 1 ? "file" : "files"} shown (partial)`
            : items.length === 1
              ? "1 changed file"
              : `${items.length} changed files`}
        </strong>
        {!partial && (
          <button type="button" onClick={() => setShowFullDiff((current) => !current)}>
            {showingFullDiff ? "Back to preview" : "View full diff"}
          </button>
        )}
      </div>
      {showingFullDiff ? (
        <DiffFiles files={files} />
      ) : (
        <div className="diff-explorer-layout [display:grid] [grid-template-columns:minmax(180px,_260px)_minmax(0,_1fr)] [align-items:start] [gap:14px] [@container((max-width:_960px))]:[grid-template-columns:1fr]">
          <div className="diff-explorer-files [position:sticky] [top:0] [max-height:min(70vh,_720px)] [overflow:auto] [border:1px_solid_var(--border)] [border-radius:var(--radius-md)] [background:var(--base)] [&_button]:[display:grid] [&_button]:[grid-template-columns:18px_minmax(0,_1fr)_auto_auto] [&_button]:[align-items:center] [&_button]:[gap:7px] [&_button]:[width:100%] [&_button]:[padding:8px_9px] [&_button]:[border-bottom:1px_solid_var(--border-variant)] [&_button]:[color:var(--text)] [&_button]:[text-align:left] [&_button:last-child]:[border-bottom:none] [&_button:hover]:[background:var(--surface)] [&_button.active]:[background:var(--surface)] [&_button.active]:[box-shadow:inset_2px_0_0_var(--text)] [&_code]:[overflow:hidden] [&_code]:[text-overflow:ellipsis] [&_code]:[white-space:nowrap] [&_code]:[font-size:var(--fs-xs)] [@container((max-width:_960px))]:[position:static] [@container((max-width:_960px))]:[max-height:220px]" aria-label="Changed files">
            {items.map((item) => (
              <button
                type="button"
                key={item.key}
                className={item.key === activeKey ? "active" : ""}
                aria-pressed={item.key === activeKey}
                onClick={() => setSelectedKey(item.key)}
              >
                <span className={`diff-file-status [font-family:var(--mono)] [font-size:var(--fs-xs)] [font-weight:var(--fw-semibold)] [color:var(--muted)] [&.status-add]:[color:var(--accent-green)] [&.status-delete]:[color:var(--accent-red)] [&.status-rename]:[color:var(--accent-blue)] [&.status-copy]:[color:var(--accent-blue)] status-${item.file.type}`}>
                  {fileStatus(item.file)}
                </span>
                <code title={formatDiffFilePath(item.file)}>{formatDiffFilePath(item.file)}</code>
                <span className="diff-explorer-stat [font-family:var(--mono)] [font-size:var(--fs-2xs)] diff-stat-add [color:var(--accent-green)]">+{item.changes.additions}</span>
                <span className="diff-explorer-stat [font-family:var(--mono)] [font-size:var(--fs-2xs)] diff-stat-del [color:var(--accent-red)]">−{item.changes.deletions}</span>
              </button>
            ))}
          </div>
          <div className={`${DIFF_CLASS_NAME} diff-explorer-preview [min-width:0]`}>
            {selected && (
              <DiffFileCard
                key={selected.key}
                file={selected.file}
                defaultExpanded
              />
            )}
          </div>
        </div>
      )}
    </div>
  );
}
