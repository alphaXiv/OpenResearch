// Shared source-code block: line-number gutter + refractor-highlighted
// content, used by the repo file viewer and the Artifacts tab preview. Style
// scoping note: syntax token colors apply under a `.file-view` ancestor.
import { useLayoutEffect, useMemo, useRef, useState } from "react";
import { detectSyntaxLanguageFromFilePath } from "../syntaxLanguage";
import { highlight } from "../syntaxHighlight";

export function CodeView({
  text,
  path,
  highlightLine,
}: {
  text: string;
  path: string;
  /** 1-based line to scroll to and highlight (from a `file:line` chip). */
  highlightLine?: number;
}) {
  const rendered = useMemo(
    () => highlight(text, detectSyntaxLanguageFromFilePath(path)),
    [text, path],
  );
  // One number per source line; a trailing newline ends a line, it doesn't
  // start an empty one.
  const lineCount = text ? text.split("\n").length - (text.endsWith("\n") ? 1 : 0) : 0;

  const codeRef = useRef<HTMLPreElement>(null);
  const bandRef = useRef<HTMLDivElement>(null);
  // Highlight band geometry, measured from the code's real font metrics so it
  // lands on the same row as the gutter number (both share padding/line-height).
  const [band, setBand] = useState<{ top: number; height: number } | null>(null);

  useLayoutEffect(() => {
    const code = codeRef.current;
    if (!highlightLine || !code || lineCount === 0) {
      setBand(null);
      return;
    }
    const target = Math.min(Math.max(Math.trunc(highlightLine), 1), lineCount);
    const cs = getComputedStyle(code);
    const padTop = Number.parseFloat(cs.paddingTop) || 0;
    const lineH = Number.parseFloat(cs.lineHeight) || 0;
    if (!lineH) {
      setBand(null);
      return;
    }
    setBand({ top: padTop + (target - 1) * lineH, height: lineH });
    // `rendered` is a dep so the band re-measures/re-scrolls once new file
    // content has laid out, not against the previous file's metrics.
  }, [highlightLine, lineCount, rendered]);

  // Center the highlighted line in the scroll viewport once its band exists.
  useLayoutEffect(() => {
    if (band) bandRef.current?.scrollIntoView({ block: "center" });
  }, [band]);

  return (
    <div className="file-view-codewrap flex items-start min-w-max relative">
      {/* No numbers for an empty file — an empty gutter is just a stray
          bordered strip. */}
      {lineCount > 0 && (
        <pre className="file-view-gutter m-0 pt-3.5 pb-3.5 font-mono text-sm leading-[1.55] pl-3.5 pr-2.5 text-right text-muted select-none sticky left-0 bg-background border-r border-r-border-variant shrink-0" aria-hidden="true">
          {Array.from({ length: lineCount }, (_, i) => i + 1).join("\n")}
        </pre>
      )}
      <pre className="file-view-code m-0 pt-3.5 pb-3.5 font-mono text-sm leading-[1.55] pl-4 pr-4 [tab-size:4] min-w-max" ref={codeRef}>
        <code>{rendered}</code>
      </pre>
      {band && (
        <div
          ref={bandRef}
          className="file-view-line-highlight absolute left-0 right-0 pointer-events-none bg-[color-mix(in_srgb,_var(--primary)_16%,_transparent)] shadow-[inset_2px_0_0_var(--primary)]"
          style={{ top: band.top, height: band.height }}
          aria-hidden="true"
        />
      )}
    </div>
  );
}
