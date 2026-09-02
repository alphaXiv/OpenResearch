import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
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
import { fmtBytes, fmtNumber } from "../api";
const DIFF_CLASS_NAME = [
  "openresearch-diff flex flex-col gap-4",
  "[&_.openresearch-diff-file]:[--diff-background-color:var(--base)]",
  "[&_.openresearch-diff-file]:[--diff-text-color:var(--text)]",
  "[&_.openresearch-diff-file]:[--diff-font-family:var(--mono)]",
  "[&_.openresearch-diff-file]:[--diff-selection-text-color:var(--primary)]",
  "[&_.openresearch-diff-file]:[--diff-selection-background-color:var(--color-diff-selection)]",
  "[&_.openresearch-diff-file]:[--diff-gutter-selected-text-color:var(--diff-selection-text-color)]",
  "[&_.openresearch-diff-file]:[--diff-gutter-selected-background-color:var(--color-diff-gutter-selection)]",
  "[&_.openresearch-diff-file]:[--diff-code-selected-text-color:var(--diff-selection-text-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-selected-background-color:var(--diff-selection-background-color)]",
  "[&_.openresearch-diff-file]:[--diff-gutter-insert-text-color:var(--accent-green)]",
  "[&_.openresearch-diff-file]:[--diff-gutter-insert-background-color:var(--color-diff-insert-gutter)]",
  "[&_.openresearch-diff-file]:[--diff-gutter-delete-text-color:var(--accent-red)]",
  "[&_.openresearch-diff-file]:[--diff-gutter-delete-background-color:var(--color-diff-delete-gutter)]",
  "[&_.openresearch-diff-file]:[--diff-code-insert-text-color:var(--diff-text-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-insert-background-color:var(--color-diff-insert-code)]",
  "[&_.openresearch-diff-file]:[--diff-code-delete-text-color:var(--diff-text-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-delete-background-color:var(--color-diff-delete-code)]",
  "[&_.openresearch-diff-file]:[--diff-code-insert-edit-text-color:var(--diff-text-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-insert-edit-background-color:var(--color-diff-insert-edit)]",
  "[&_.openresearch-diff-file]:[--diff-code-delete-edit-text-color:var(--diff-text-color)]",
  "[&_.openresearch-diff-file]:[--diff-code-delete-edit-background-color:var(--color-diff-delete-edit)]",
  "[&_.openresearch-diff-file]:[--diff-omit-gutter-line-color:var(--color-diff-omit-gutter)]",
  "[&_.openresearch-diff-file]:w-full [&_.openresearch-diff-file]:text-sm",
  "[&_.openresearch-diff-file]:leading-[1.55] [&_.openresearch-diff-file.diff-unified]:table-auto",
  "[&_.openresearch-diff-file.diff-unified_col.diff-gutter-col:first-child]:collapse",
  "[&_.openresearch-diff-file.diff-unified_col.diff-gutter-col:first-child]:w-0",
  "[&_.openresearch-diff-file.diff-unified_col.diff-gutter-col:nth-child(2)]:w-[1%]",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:first-child]:hidden",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:sticky",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:start-0",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:z-1",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:w-[1%]",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:pt-0 [&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:pe-2.5 [&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:pb-0 [&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:ps-3.5",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:whitespace-nowrap",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:text-end",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:text-diff-gutter-text",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:border-e [&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:border-e-border",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:select-none",
  "[&_.openresearch-diff-file.diff-unified_.diff-line_>_td:nth-child(2)]:cursor-default",
  "[&_.openresearch-diff-file_.diff-line]:leading-[1.55]",
  "[&_.openresearch-diff-file_.diff-line:has(.diff-code-insert)]:bg-diff-insert-code",
  "[&_.openresearch-diff-file_.diff-line:has(.diff-code-delete)]:bg-diff-delete-code",
  "[&_.openresearch-diff-file_.diff-code]:py-0 [&_.openresearch-diff-file_.diff-code]:px-4",
  "[&_.openresearch-diff-file_.diff-code]:whitespace-pre",
  "[&_.openresearch-diff-file_.diff-code]:break-normal",
  "[&_.openresearch-diff-file_.diff-code]:wrap-normal",
  "[&_.openresearch-diff-file_.diff-hunk_+_.diff-hunk_.diff-line:first-child_>_td]:border-t [&_.openresearch-diff-file_.diff-hunk_+_.diff-hunk_.diff-line:first-child_>_td]:border-t-border",
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

export function TruncatedDiffNotice({
  bytesRead,
  byteLimit,
}: {
  bytesRead: number;
  byteLimit: number;
}) {
  return (
    <div className="truncated-notice border border-accent-amber rounded-md bg-accent-amber-subtle py-3 px-3.5 text-sm [&_h4]:mt-0 [&_h4]:mx-0 [&_h4]:mb-1 [&_h4]:text-sm [&_h4]:text-accent-amber [&_p]:m-0 [&_p]:text-subtext">
      <h4>{m.git_diff_diff_preview_truncated()}</h4>
      <p>
        {m.git_diff_truncated_detail({ limit: ltr(fmtBytes(byteLimit)), read: ltr(fmtBytes(bytesRead)) })}
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
    <section className={`diff-file-card overflow-hidden border border-border rounded-md bg-background [&.expanded_.diff-file-header]:border-b [&.expanded_.diff-file-header]:border-b-border ${expanded ? "expanded" : ""}`}>
      <button
        className="diff-file-header sticky top-0 z-10 flex items-center justify-between gap-3 w-full text-start py-2 px-3 bg-canvas cursor-pointer [&_.chev]:text-muted [&_.chev]:text-xs [&_.chev]:shrink-0 [&_.chev]:w-3 [&_.path]:flex [&_.path]:items-center [&_.path]:gap-2 [&_.path]:min-w-0 [&_.path]:flex-1 [&_.path_code]:min-w-0 [&_.path_code]:flex-1 [&_.path_code]:overflow-hidden [&_.path_code]:text-ellipsis [&_.path_code]:whitespace-nowrap [&_.path_code]:font-mono [&_.path_code]:text-xs [&_.path_code]:font-semibold [&_.path_code]:text-text [&_.stats]:flex [&_.stats]:items-center [&_.stats]:gap-2 [&_.stats]:shrink-0 [&_.stats]:font-mono [&_.stats]:text-xs [&_.stats]:font-medium [&_.stats]:tabular-nums"
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
          <span className="diff-stat-add text-accent-green">+{additions}</span>
          <span className="diff-stat-del text-accent-red">−{deletions}</span>
        </span>
      </button>
      {expanded &&
        (file.hunks.length === 0 ? (
          <div className="diff-empty py-2 px-3 text-muted text-sm">{m.git_diff_no_textual_diff_for_this_file()}</div>
        ) : (
          <div className="diff-file-body overflow-x-auto bg-background">
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
  if (parsed.failed) return <div className="diff-empty py-2 px-3 text-muted text-sm">{m.git_diff_unable_to_parse_this_diff()}</div>;
  if (parsed.files.length === 0) return <div className="diff-empty py-2 px-3 text-muted text-sm">{m.git_diff_no_changes()}</div>;
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
      <div className="diff-empty py-2 px-3 text-muted text-sm">
        {partial ? m.git_diff_no_complete_preview() : m.git_diff_parse_failed()}
      </div>
    );
  }
  if (items.length === 0) return <div className="diff-empty py-2 px-3 text-muted text-sm">{m.git_diff_no_changes()}</div>;

  return (
    <div className="diff-explorer @container">
      <div className="diff-explorer-toolbar flex items-center justify-between gap-3 mb-2.5 text-sm [&_button]:py-0.5 [&_button]:px-0 [&_button]:text-muted [&_button]:text-sm [&_button]:font-medium [&_button:hover]:text-text [&_button:hover]:underline [&_button:hover]:underline-offset-2">
        <strong>
          {partial
            ? items.length === 1 ? m.git_diff_one_file_partial() : m.git_diff_files_partial({ count: fmtNumber(items.length) })
            : items.length === 1
              ? m.git_diff_one_changed_file()
              : m.git_diff_changed_file_count({ count: fmtNumber(items.length) })}
        </strong>
        {!partial && (
          <button type="button" onClick={() => setShowFullDiff((current) => !current)}>
            {showingFullDiff ? m.git_diff_back_to_preview() : m.git_diff_view_full()}
          </button>
        )}
      </div>
      {showingFullDiff ? (
        <DiffFiles files={files} />
      ) : (
        <div className="diff-explorer-layout grid grid-cols-[minmax(180px,_260px)_minmax(0,_1fr)] items-start gap-3.5 [@container((max-width:_960px))]:grid-cols-1">
          <div className="diff-explorer-files sticky top-0 max-h-[min(70vh,_720px)] overflow-auto border border-border rounded-md bg-background [&_button]:grid [&_button]:grid-cols-[18px_minmax(0,_1fr)_auto_auto] [&_button]:items-center [&_button]:gap-[7px] [&_button]:w-full [&_button]:py-2 [&_button]:px-[9px] [&_button]:border-b [&_button]:border-b-border-variant [&_button]:text-text [&_button]:text-start [&_button:last-child]:border-b-0 [&_button:hover]:bg-surface [&_button.active]:bg-surface [&_button.active]:shadow-diff-active [&_code]:overflow-hidden [&_code]:text-ellipsis [&_code]:whitespace-nowrap [&_code]:text-xs [@container((max-width:_960px))]:static [@container((max-width:_960px))]:max-h-55" aria-label={m.git_diff_changed_files()}>
            {items.map((item) => (
              <button
                type="button"
                key={item.key}
                className={item.key === activeKey ? "active" : ""}
                aria-pressed={item.key === activeKey}
                onClick={() => setSelectedKey(item.key)}
              >
                <span className={`diff-file-status font-mono text-xs font-medium text-muted [&.status-add]:text-accent-green [&.status-delete]:text-accent-red [&.status-rename]:text-accent-blue [&.status-copy]:text-accent-blue status-${item.file.type}`}>
                  {fileStatus(item.file)}
                </span>
                <code title={formatDiffFilePath(item.file)}>{formatDiffFilePath(item.file)}</code>
                <span className="diff-explorer-stat font-mono text-xs diff-stat-add text-accent-green">+{item.changes.additions}</span>
                <span className="diff-explorer-stat font-mono text-xs diff-stat-del text-accent-red">−{item.changes.deletions}</span>
              </button>
            ))}
          </div>
          <div className={`${DIFF_CLASS_NAME} diff-explorer-preview min-w-0`}>
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
