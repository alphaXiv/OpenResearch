// Layout the read-only code view and the editor's highlighted overlay must
// agree on: the two render the same file side by side (the overlay sits under a
// transparent textarea), so any divergence in font metrics, wrapping or gutter
// width shows up as text that doesn't line up.

// Preflight isn't imported (see tailwind.css), so the UA's
// `pre, code { font-family: monospace }` beats an inherited font — every code
// element needs these spelled out rather than relying on an ancestor.
export const CODE_TEXT_CLASS_NAME = "font-mono text-sm leading-[1.55] [tab-size:4]";

export const CODE_WRAP_CLASS_NAME = "whitespace-pre-wrap break-words";

export const CODE_GUTTER_CLASS_NAME = "file-view-gutter text-right text-muted select-none";

/** Column positions in `ch`, so they track the mono font: the rule sits at
 * `ruleCh` (the number is right-aligned just inside it), code starts at
 * `codeCh` — 2ch past the rule, which is where both views pad their code. Only
 * meaningful on an element with CODE_TEXT_CLASS_NAME. */
export function codeGutter(lineCount: number): { ruleCh: number; codeCh: number } {
  const ruleCh = String(lineCount).length + 2;
  // The 2ch gap is spelled `pl-[2ch]` on CodeView's code column.
  return { ruleCh, codeCh: ruleCh + 2 };
}
