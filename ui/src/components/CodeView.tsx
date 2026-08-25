// Shared source-code block: line-number gutter + refractor-highlighted
// content, used by the repo file viewer and the Artifacts tab preview. Style
// scoping note: syntax token colors apply under a `.file-view` ancestor.
import { useEffect, useMemo, useRef } from "react";
import {
  CODE_GUTTER_CLASS_NAME,
  CODE_TEXT_CLASS_NAME,
  CODE_WRAP_CLASS_NAME,
  codeGutter,
} from "../codeLayout";
import { detectSyntaxLanguageFromFilePath } from "../syntaxLanguage";
import { highlightLines, isBlankLine } from "../syntaxHighlight";

export function CodeView({
  text,
  path,
  highlightLine,
  scrollRequest,
  onScrollRequestHandled,
}: {
  text: string;
  path: string;
  /** 1-based line to scroll to and highlight (from a `file:line` chip). */
  highlightLine?: number;
  scrollRequest?: number;
  onScrollRequestHandled?: () => void;
}) {
  // A trailing newline ends a line, it doesn't start an empty one. Empty files
  // render no rows at all — a lone gutter is just a stray bordered strip.
  const lines = useMemo(() => {
    if (!text) return [];
    // CR is a segment break under pre-wrap, so any CR-bearing file would
    // render every row double-height.
    const source = text.replace(/\r\n?/g, "\n");
    const all = highlightLines(source, detectSyntaxLanguageFromFilePath(path));
    return source.endsWith("\n") ? all.slice(0, -1) : all;
  }, [text, path]);
  const targetLine =
    highlightLine && lines.length > 0
      ? Math.min(Math.max(Math.trunc(highlightLine), 1), lines.length)
      : undefined;

  const targetRowRef = useRef<HTMLDivElement>(null);

  // Center the highlighted line in the scroll viewport. Passive, unlike the
  // editor's: this scrolls the shared viewport, which FileViewer restores in a
  // layout effect that runs after ours and would otherwise win.
  useEffect(() => {
    if (scrollRequest === undefined) return;
    if (targetLine) {
      targetRowRef.current?.scrollIntoView({ block: "center" });
      onScrollRequestHandled?.();
    } else if (lines.length === 0) {
      onScrollRequestHandled?.();
    }
  }, [lines.length, onScrollRequestHandled, scrollRequest, targetLine]);

  const { ruleCh } = codeGutter(lines.length);

  // Rows are memoized so an unrelated parent re-render (chat streaming) doesn't
  // reconcile every line of the open file.
  const rows = useMemo(
    () =>
      lines.map((line, i) => (
        <div
          key={i}
          ref={i + 1 === targetLine ? targetRowRef : undefined}
          className={`file-view-line flex items-stretch ${
            i + 1 === targetLine
              ? "file-view-line-highlight bg-accent-blue-subtle shadow-[inset_2px_0_0_var(--accent-blue)]"
              : ""
          }`}
        >
          {/* The number is generated content so a drag-selection over the
              code never picks it up. */}
          <span
            data-line={i + 1}
            className={`${CODE_GUTTER_CLASS_NAME} before:content-[attr(data-line)] shrink-0 pr-[1ch]`}
            style={{ width: `${ruleCh}ch` }}
            aria-hidden="true"
          />
          <code
            className={`file-view-code flex-1 min-w-0 pl-[2ch] pr-4 ${CODE_TEXT_CLASS_NAME} ${CODE_WRAP_CLASS_NAME}`}
          >
            {/* An empty <code> serializes to nothing, dropping blank lines
                from a copied selection. */}
            {isBlankLine(line) ? <br /> : line}
          </code>
        </div>
      )),
    [lines, ruleCh, targetLine],
  );

  return (
    <div className={`file-view-codewrap relative py-3.5 ${CODE_TEXT_CLASS_NAME}`}>
      {lines.length > 0 && (
        <div
          className="absolute left-0 top-0 bottom-0 border-r border-r-border-variant pointer-events-none"
          style={{ width: `${ruleCh}ch` }}
          aria-hidden="true"
        />
      )}
      {rows}
    </div>
  );
}
