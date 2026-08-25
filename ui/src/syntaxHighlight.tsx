// Shared refractor highlighting used by the file viewer (path → language) and
// chat markdown code blocks (fence info → language). Both render the resulting
// hast into <span class="token …"> nodes styled by the .token theme in
// Tailwind utilities on the markdown container.

import type { ReactNode } from "react";
import { refractor } from "refractor";
import latex from "refractor/latex";

// The common bundle stops short of latex, and .tex is a first-class file here.
refractor.register(latex);

interface HastNode {
  type: string;
  value?: string;
  properties?: { className?: string[] };
  children?: HastNode[];
}

/** Tokenize `code`, best-effort: null when the language isn't registered, the
 * input is too large, or tokenizing throws — callers fall back to plain text. */
function tokenize(code: string, lang: string | null, maxBytes: number): HastNode[] | null {
  if (!lang || !refractor.registered(lang) || code.length > maxBytes) return null;
  try {
    return refractor.highlight(code, lang).children as HastNode[];
  } catch {
    return null;
  }
}

function hastToReact(node: HastNode, key: number): ReactNode {
  if (node.type === "text") return node.value ?? "";
  if (node.type !== "element") return null;
  return (
    <span key={key} className={(node.properties?.className ?? []).join(" ")}>
      {(node.children ?? []).map(hastToReact)}
    </span>
  );
}

/** Highlight `code` in `lang`, best-effort: returns the raw string when the
 * language isn't registered, the input is too large, or tokenizing throws. */
export function highlight(code: string, lang: string | null, maxBytes = 300_000): ReactNode {
  return tokenize(code, lang, maxBytes)?.map(hastToReact) ?? code;
}

/** Highlight `code` and split it into one node per source line, so callers can
 * pair each line with a gutter number that stays put when the line wraps. */
export function highlightLines(code: string, lang: string | null, maxBytes = 300_000): ReactNode[] {
  const tokens = tokenize(code, lang, maxBytes);
  if (!tokens) return code.split("\n");
  const lines: ReactNode[][] = [];
  let line: ReactNode[] = [];
  // Class names of the token spans enclosing the text being emitted; a token
  // that straddles a newline is reopened on the next line.
  const open: string[] = [];
  let key = 0;
  const emit = (text: string) => {
    let node: ReactNode = text;
    for (let i = open.length - 1; i >= 0; i--) {
      node = <span key={key++} className={open[i]}>{node}</span>;
    }
    line.push(node);
  };
  const walk = (node: HastNode) => {
    if (node.type === "text") {
      (node.value ?? "").split("\n").forEach((part, i) => {
        if (i > 0) {
          lines.push(line);
          line = [];
        }
        if (part) emit(part);
      });
      return;
    }
    if (node.type !== "element") return;
    open.push((node.properties?.className ?? []).join(" "));
    (node.children ?? []).forEach(walk);
    open.pop();
  };
  tokens.forEach(walk);
  lines.push(line);
  return lines;
}

/** A `highlightLines` entry with no content — an empty source line. */
export function isBlankLine(line: ReactNode): boolean {
  return Array.isArray(line) ? line.length === 0 : line === "";
}
