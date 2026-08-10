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
    <div className="file-view-codewrap [display:flex] [align-items:flex-start] [min-width:max-content] [position:relative]">
      {/* No numbers for an empty file — an empty gutter is just a stray
          bordered strip. */}
      {lineCount > 0 && (
        <pre className="file-view-gutter [margin:0] [padding-top:14px] [padding-bottom:14px] [font-family:var(--mono)] [font-size:var(--fs-sm)] [line-height:1.55] [padding-left:14px] [padding-right:10px] [text-align:right] [color:var(--muted)] [user-select:none] [position:sticky] [left:0] [background:var(--base)] [border-right:1px_solid_var(--border-variant)] [flex-shrink:0]" aria-hidden="true">
          {Array.from({ length: lineCount }, (_, i) => i + 1).join("\n")}
        </pre>
      )}
      <pre className="file-view-code [margin:0] [padding-top:14px] [padding-bottom:14px] [font-family:var(--mono)] [font-size:var(--fs-sm)] [line-height:1.55] [padding-left:16px] [padding-right:16px] [tab-size:4] [min-width:max-content]" ref={codeRef}>
        <code>{rendered}</code>
      </pre>
      {band && (
        <div
          ref={bandRef}
          className="file-view-line-highlight [position:absolute] [left:0] [right:0] [pointer-events:none] [background:color-mix(in_srgb,_var(--primary)_16%,_transparent)] [box-shadow:inset_2px_0_0_var(--primary)]"
          style={{ top: band.top, height: band.height }}
          aria-hidden="true"
        />
      )}
    </div>
  );
}
