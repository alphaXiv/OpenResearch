// Inline source editor: a transparent <textarea> layered over the same
// refractor-highlighted lines CodeView renders read-only, so syntax colors stay
// live while typing. It IS the view for editable files — there's no separate
// mode, you just click and type. The textarea owns input, caret and selection;
// the highlighted overlay is scroll-synced to it. Line numbers are absolutely
// positioned out of the overlay's line boxes: anything in flow there is a wrap
// opportunity the textarea doesn't have, which desyncs the two layers.
// Token colors apply under a `.file-view` ancestor (see CodeView).
import { useLayoutEffect, useMemo, useRef } from "react";
import {
  CODE_GUTTER_CLASS_NAME,
  CODE_TEXT_CLASS_NAME,
  CODE_WRAP_CLASS_NAME,
  codeGutter,
} from "../codeLayout";
import { detectSyntaxLanguageFromFilePath } from "../syntaxLanguage";
import { highlightLines, isBlankLine } from "../syntaxHighlight";

export function CodeEditor({
  value,
  onChange,
  onSave,
  onBlur,
  path,
  highlightLine,
  scrollRequest,
  onScrollRequestHandled,
}: {
  value: string;
  onChange: (next: string) => void;
  /** Cmd/Ctrl+S while focused — the viewer maps this to save. */
  onSave: () => void;
  /** Focus left the editor — the viewer saves any pending edit. */
  onBlur?: () => void;
  path: string;
  /** 1-based line to scroll to and place the caret on (from a `file:line` chip). */
  highlightLine?: number;
  /** Bumped when a `file:line` chip reopens the already-open file at a new line,
   * so the caret re-navigates even though `path` didn't change. */
  scrollRequest?: number;
  onScrollRequestHandled?: () => void;
}) {
  // A trailing newline opens a new (empty) line the caret can sit on, so unlike
  // the read-only view every "\n" gets a row.
  const lines = useMemo(
    () => highlightLines(value, detectSyntaxLanguageFromFilePath(path)),
    [value, path],
  );
  const { ruleCh, codeCh } = codeGutter(lines.length);

  const taRef = useRef<HTMLTextAreaElement>(null);
  const overlayRef = useRef<HTMLDivElement>(null);

  // Keep the highlighted layer pinned to the textarea's scroll.
  const syncScroll = () => {
    const ta = taRef.current;
    if (ta && overlayRef.current) overlayRef.current.scrollTop = ta.scrollTop;
  };
  // Re-sync after content changes relayout (e.g. a newline shifts scrollHeight).
  useLayoutEffect(syncScroll, [value]);

  // On open via a `file:line` chip, park the caret on that line and center it.
  // Re-runs when the file changes (path) or a new chip targets the open file
  // (scrollRequest) — which is also what makes it land: the first run sees the
  // draft before FileViewer's passive effect has seeded it, and clearing the
  // request re-runs this against real content.
  useLayoutEffect(() => {
    const ta = taRef.current;
    if (!ta || !highlightLine) return;
    const text = value.split("\n");
    const target = Math.min(Math.max(Math.trunc(highlightLine), 1), text.length);
    let caret = 0;
    for (let i = 0; i < target - 1; i++) caret += text[i].length + 1;
    ta.setSelectionRange(caret, caret);
    // The overlay wraps exactly like the textarea, so its row is where the line
    // actually sits — line-height arithmetic would miss by every wrap.
    const row = overlayRef.current?.querySelector<HTMLElement>(`[data-line="${target}"]`);
    if (row) ta.scrollTop = Math.max(0, row.offsetTop - ta.clientHeight / 2);
    syncScroll();
    onScrollRequestHandled?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, scrollRequest]);

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
      e.preventDefault();
      onSave();
      return;
    }
    if (e.key === "Tab") {
      e.preventDefault();
      const ta = e.currentTarget;
      const { selectionStart, selectionEnd } = ta;
      const next = value.slice(0, selectionStart) + "\t" + value.slice(selectionEnd);
      onChange(next);
      // Restore the caret just past the inserted tab once React re-renders.
      requestAnimationFrame(() => {
        ta.selectionStart = ta.selectionEnd = selectionStart + 1;
      });
    }
  };

  // Both layers must reserve the scrollbar, or the textarea is narrower than the
  // overlay on classic-scrollbar platforms and the two wrap at different columns.
  const layerClassName = `absolute inset-0 m-0 py-3.5 pe-4 ${CODE_TEXT_CLASS_NAME} ${CODE_WRAP_CLASS_NAME} [scrollbar-gutter:stable]`;

  return (
    <div className={`file-view-editwrap relative h-full min-h-0 ${CODE_TEXT_CLASS_NAME}`}>
      <div
        className="absolute start-0 top-0 bottom-0 border-e border-e-border-variant pointer-events-none"
        style={{ width: `${ruleCh}ch` }}
        aria-hidden="true"
     />
      <div
        ref={overlayRef}
        className={`file-view-code ${layerClassName} overflow-hidden pointer-events-none`}
        aria-hidden="true"
      >
        {lines.map((line, i) => (
          <div
            key={i}
            /* The scroll target for `file:line` — measured, so it's on the row. */
            data-line={i + 1}
            className="relative"
            style={{ paddingInlineStart: `${codeCh}ch` }}
          >
            <span
              className={`${CODE_GUTTER_CLASS_NAME} absolute start-0 pe-[1ch]`}
              style={{ width: `${ruleCh}ch` }}
            >
              {i + 1}
            </span>
            {/* Out-of-flow numbers leave a blank line with no line box at all. */}
            {isBlankLine(line) ? <br /> : line}
          </div>
        ))}
      </div>
      <textarea
        ref={taRef}
        className={`file-view-editarea ${layerClassName} overflow-y-auto overflow-x-hidden resize-none border-0 bg-transparent text-transparent caret-text outline-none`}
        style={{ paddingInlineStart: `${codeCh}ch` }}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onScroll={syncScroll}
        onKeyDown={onKeyDown}
        onBlur={onBlur}
        spellCheck={false}
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
     />
    </div>
  );
}
