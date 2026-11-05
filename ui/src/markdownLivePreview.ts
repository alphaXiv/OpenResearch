/**
 * Markdown live preview for CodeMirror, ported from alphaXiv's editor. The
 * markdown text is the source of truth; formatting is a decoration layer that
 * hides syntax marks until the selection touches them.
 *
 * @module
 */
import { insertNewlineContinueMarkup } from "@codemirror/lang-markdown";
import { HighlightStyle, syntaxHighlighting, syntaxTree } from "@codemirror/language";
import {
  EditorSelection,
  Prec,
  StateEffect,
  StateField,
  type EditorState,
  type Extension,
  type Range,
  type Transaction,
} from "@codemirror/state";
import { Decoration, EditorView, keymap, WidgetType, type DecorationSet } from "@codemirror/view";
import { tags as t } from "@lezer/highlight";
import type { SyntaxNode, Tree } from "@lezer/common";
import katex from "katex";

const setPointerSelecting = StateEffect.define<boolean>();

/** True while the primary mouse button is down in the editor. */
const pointerSelecting = StateField.define<boolean>({
  create: () => false,
  update: (value, transaction) => {
    for (const effect of transaction.effects) if (effect.is(setPointerSelecting)) value = effect.value;
    return value;
  },
});

/** Tracks mouse drags so syntax reveals wait for the button to come up: marks
 * appearing mid-drag would shift the text out from under the pointer. */
export const pointerSelection: Extension = [
  pointerSelecting,
  EditorView.domEventObservers({
    mousedown(event, view) {
      if (event.button !== 0) return;
      view.dispatch({ effects: setPointerSelecting.of(true) });
      // Deferred past CodeMirror's own mouseup, which sets the final selection.
      const release = () =>
        setTimeout(() => {
          if (!view.dom.isConnected) return;
          const selection = selectionOverMarks(view.state);
          view.dispatch({
            effects: setPointerSelecting.of(false),
            selection: selection.eq(view.state.selection) ? undefined : selection,
          });
        });
      window.addEventListener("mouseup", release, { once: true, capture: true });
    },
  }),
];

/** The text between a formatted node's opening and closing marks. */
function markedText(node: SyntaxNode): { from: number; to: number } | null {
  const markName = node.name === "Link" ? "LinkMark" : DELIMITED[node.name]?.mark;
  if (!markName) return null;
  const marks = node.getChildren(markName);
  const [open, close] = node.name === "Link" ? marks : [marks[0], marks[marks.length - 1]];
  if (!open || !close || close === open || close.from <= open.to) return null;
  return { from: open.to, to: close.from };
}

/** Grows each range that covers a formatted span's text to take in its marks,
 * which were hidden while the range was drawn. */
export function selectionOverMarks(state: EditorState): EditorSelection {
  const tree = syntaxTree(state);
  return EditorSelection.create(
    state.selection.ranges.map((range) => {
      if (range.empty) return range;
      let { from, to } = range;
      // Children leave first, so a nested span's growth can cover its parent's text.
      tree.iterate({
        from,
        to,
        enter: () => undefined,
        leave: (node) => {
          const text = markedText(node.node);
          if (!text || from > text.from || to < text.to) return;
          from = Math.min(from, node.from);
          to = Math.max(to, node.to);
        },
      });
      return range.anchor <= range.head ? EditorSelection.range(from, to) : EditorSelection.range(to, from);
    }),
    state.selection.mainIndex,
  );
}

/** Whether selection-dependent decorations must be rebuilt for a transaction. */
export function revealChanged(transaction: Transaction): boolean {
  if (transaction.docChanged) return true;
  if (transaction.effects.some((effect) => effect.is(setPointerSelecting) && !effect.value)) return true;
  return transaction.selection !== undefined && !transaction.state.field(pointerSelecting);
}

/** A range of the selection touches `[from, to]`. */
export function revealedSpan(state: EditorState, from: number, to: number): boolean {
  return state.selection.ranges.some((range) => range.from <= to && range.to >= from);
}

/** A range of the selection touches any line `[from, to]` spans. */
function revealedLines(state: EditorState, from: number, to: number): boolean {
  const { doc } = state;
  const first = doc.lineAt(from).number;
  const last = doc.lineAt(to).number;
  return state.selection.ranges.some(
    (range) => doc.lineAt(range.from).number <= last && doc.lineAt(range.to).number >= first,
  );
}

export interface MathToken {
  from: number;
  to: number;
  latex: string;
  block: boolean;
}

/**
 * Inline `$...$` and block `$$...$$` spans. Escaped dollars are skipped and
 * unterminated spans ignored; an inline span with whitespace touching either
 * `$` is prose (`$5 and $10`), matching remark-math's Pandoc rule.
 */
export function findMathTokens(text: string): MathToken[] {
  const tokens: MathToken[] = [];
  const n = text.length;
  let i = 0;
  while (i < n) {
    if (text[i] !== "$" || text[i - 1] === "\\") {
      i++;
      continue;
    }
    if (text[i + 1] === "$") {
      const close = text.indexOf("$$", i + 2);
      if (close === -1) break;
      const latex = text.slice(i + 2, close).trim();
      const lineStart = text.lastIndexOf("\n", i - 1) + 1;
      const lineEnd = text.indexOf("\n", close + 2);
      // Prose sharing a line with `$$` makes it inline math, as in remark-math.
      const block = !text.slice(lineStart, i).trim() && !text.slice(close + 2, lineEnd === -1 ? n : lineEnd).trim();
      if (latex) tokens.push({ from: i, to: close + 2, latex, block });
      i = close + 2;
      continue;
    }
    const newline = text.indexOf("\n", i + 1);
    const lineEnd = newline === -1 ? n : newline;
    let close = -1;
    for (let j = i + 1; j < lineEnd; j++) {
      if (text[j] === "$" && text[j - 1] !== "\\") {
        close = j;
        break;
      }
    }
    if (close === -1) {
      i++;
      continue;
    }
    const raw = text.slice(i + 1, close);
    // remark-math still pairs both delimiters before rejecting, so skip past the close.
    if (/^\s|\s$/.test(raw)) {
      i = close + 1;
      continue;
    }
    const latex = raw.trim();
    if (latex) tokens.push({ from: i, to: close + 1, latex, block: false });
    i = close + 1;
  }
  return tokens;
}

class KatexWidget extends WidgetType {
  readonly latex: string;
  readonly block: boolean;

  constructor(latex: string, block: boolean) {
    super();
    this.latex = latex;
    this.block = block;
  }

  override eq(other: KatexWidget) {
    return other.latex === this.latex && other.block === this.block;
  }

  override toDOM() {
    const element = document.createElement(this.block ? "div" : "span");
    element.className = this.block ? "cm-math-block" : "cm-math-inline";
    katex.render(this.latex, element, { throwOnError: false, displayMode: this.block });
    return element;
  }

  // A click lands the caret on the token and reveals its source.
  override ignoreEvent() {
    return false;
  }
}

function inCode(tree: Tree, pos: number): boolean {
  for (let node: SyntaxNode | null = tree.resolveInner(pos, 1); node; node = node.parent) {
    if (node.name === "InlineCode" || node.name === "FencedCode" || node.name === "CodeBlock") return true;
  }
  return false;
}

/** Style the content between a node's delimiter marks, hiding the marks unless
 * the selection touches the node, where they are muted instead. */
function decorateDelimited(
  state: EditorState,
  node: SyntaxNode,
  markName: string,
  className: string,
  ranges: Range<Decoration>[],
): void {
  const marks = node.getChildren(markName);
  const open = marks[0];
  const close = marks[marks.length - 1];
  const innerFrom = open ? open.to : node.from;
  const innerTo = close ? close.from : node.to;
  if (innerTo > innerFrom) ranges.push(Decoration.mark({ class: className }).range(innerFrom, innerTo));
  const decoration = revealedSpan(state, node.from, node.to)
    ? Decoration.mark({ class: "cm-lp-mark" })
    : Decoration.replace({});
  if (open) ranges.push(decoration.range(open.from, open.to));
  if (close && close !== open) ranges.push(decoration.range(close.from, close.to));
}

const DELIMITED: Record<string, { mark: string; className: string }> = {
  StrongEmphasis: { mark: "EmphasisMark", className: "cm-lp-strong" },
  Emphasis: { mark: "EmphasisMark", className: "cm-lp-em" },
  Strikethrough: { mark: "StrikethroughMark", className: "cm-lp-strike" },
  InlineCode: { mark: "CodeMark", className: "cm-lp-code" },
};

class BulletWidget extends WidgetType {
  override eq() {
    return true;
  }

  override toDOM() {
    const span = document.createElement("span");
    span.className = "cm-lp-bullet";
    span.textContent = "•";
    return span;
  }
}

const bulletWidget = new BulletWidget();

function collectFormatting(state: EditorState, tree: Tree, ranges: Range<Decoration>[]): void {
  const { doc } = state;
  tree.iterate({
    enter: (nodeRef) => {
      const { name } = nodeRef;
      const delimited = DELIMITED[name];
      if (delimited) {
        decorateDelimited(state, nodeRef.node, delimited.mark, delimited.className, ranges);
        return;
      }
      if (name === "Link") {
        const [openBracket, closeBracket] = nodeRef.node.getChildren("LinkMark");
        if (!openBracket || !closeBracket) return;
        const textFrom = openBracket.to;
        const textTo = closeBracket.from;
        if (textTo > textFrom) ranges.push(Decoration.mark({ class: "cm-lp-link" }).range(textFrom, textTo));
        if (revealedSpan(state, nodeRef.from, nodeRef.to)) return;
        if (textFrom > nodeRef.from) ranges.push(Decoration.replace({}).range(nodeRef.from, textFrom));
        if (nodeRef.to > textTo) ranges.push(Decoration.replace({}).range(textTo, nodeRef.to));
        return;
      }
      if (name.startsWith("ATXHeading")) {
        const level = name.slice("ATXHeading".length);
        ranges.push(Decoration.line({ class: `cm-lp-h${level}` }).range(doc.lineAt(nodeRef.from).from));
        const mark = nodeRef.node.getChild("HeaderMark");
        if (!mark) return;
        const end = doc.sliceString(mark.to, mark.to + 1) === " " ? mark.to + 1 : mark.to;
        // Collapsed rather than replaced: a replace widget measures on a different
        // baseline, so toggling it would shift the heading vertically.
        const revealed = revealedLines(state, nodeRef.from, nodeRef.to);
        ranges.push(
          Decoration.mark({ class: revealed ? "cm-lp-headermark" : "cm-lp-headermark-hidden" }).range(mark.from, end),
        );
        return;
      }
      if (name === "Blockquote") {
        const first = doc.lineAt(nodeRef.from).number;
        const last = doc.lineAt(nodeRef.to).number;
        for (let n = first; n <= last; n++) ranges.push(Decoration.line({ class: "cm-lp-quote" }).range(doc.line(n).from));
        return;
      }
      if (name === "QuoteMark") {
        if (revealedLines(state, nodeRef.from, nodeRef.to)) {
          ranges.push(Decoration.mark({ class: "cm-lp-mark" }).range(nodeRef.from, nodeRef.to));
        } else {
          const after = doc.sliceString(nodeRef.to, nodeRef.to + 1) === " " ? nodeRef.to + 1 : nodeRef.to;
          ranges.push(Decoration.replace({}).range(nodeRef.from, after));
        }
        return;
      }
      if (name === "ListMark") {
        const mark = doc.sliceString(nodeRef.from, nodeRef.to);
        const unordered = /^[-*+]$/.test(mark);
        if (unordered && !/^[ \t]$/.test(doc.sliceString(nodeRef.to, nodeRef.to + 1))) return;
        ranges.push(
          unordered
            ? Decoration.replace({ widget: bulletWidget }).range(nodeRef.from, nodeRef.to)
            : Decoration.mark({ class: "cm-lp-listmark" }).range(nodeRef.from, nodeRef.to),
        );
        return;
      }
      if (name === "FencedCode" || name === "CodeBlock") {
        // A bordered box like the rendered block. The ``` fences stay in the
        // document, collapsed until the block holds the selection, so reveal
        // shifts text only horizontally.
        const first = doc.lineAt(nodeRef.from).number;
        const last = doc.lineAt(nodeRef.to).number;
        const fenced = name === "FencedCode";
        const marks = fenced ? nodeRef.node.getChildren("CodeMark") : [];
        const closed = marks.length > 1;
        const codeLast = closed ? last - 1 : last;
        for (let n = first; n <= last; n++) {
          const lineFrom = doc.line(n).from;
          if (fenced && n === first) {
            ranges.push(Decoration.line({ class: "cm-lp-codeblock cm-lp-codeblock-top cm-lp-codehdr" }).range(lineFrom));
          } else if (closed && n === last) {
            ranges.push(Decoration.line({ class: "cm-lp-codefoot" }).range(lineFrom));
          } else {
            let className = "cm-lp-codeblock";
            if (!fenced && n === first) className += " cm-lp-codeblock-top";
            if (n === codeLast) className += " cm-lp-codeblock-bottom";
            ranges.push(Decoration.line({ class: className }).range(lineFrom));
          }
        }
        const revealed = revealedLines(state, nodeRef.from, nodeRef.to);
        const info = fenced ? nodeRef.node.getChild("CodeInfo") : null;
        if (info) ranges.push(Decoration.mark({ class: "cm-lp-code-lang" }).range(info.from, info.to));
        if (marks[0]) {
          ranges.push(
            Decoration.mark({ class: revealed ? "cm-lp-fencemark" : "cm-lp-fencemark-hidden" }).range(marks[0].from, marks[0].to),
          );
        }
        const close = marks[marks.length - 1];
        if (closed && close) {
          // Constant size in flow, toggled by opacity, so the foot row never changes height.
          const className = revealed ? "cm-lp-fencemark-foot cm-lp-fencemark-foot-shown" : "cm-lp-fencemark-foot";
          ranges.push(Decoration.mark({ class: className }).range(close.from, close.to));
        }
        return false;
      }
      return undefined;
    },
  });
}

const TEX_TOKEN = /\\[a-zA-Z]+|\\.|[{}]|\$\$?/g;

function highlightTex(source: string, offset: number, ranges: Range<Decoration>[]): void {
  for (const match of source.matchAll(TEX_TOKEN)) {
    const start = offset + (match.index ?? 0);
    const className = match[0][0] === "$" ? "cm-tex-dollar" : match[0][0] === "\\" ? "cm-tex-cmd" : "cm-tex-brace";
    ranges.push(Decoration.mark({ class: className }).range(start, start + match[0].length));
  }
}

function collectMath(state: EditorState, tree: Tree, tokens: MathToken[], ranges: Range<Decoration>[]): void {
  const { doc } = state;
  for (const token of tokens) {
    if (inCode(tree, token.from)) continue;
    if (token.block) {
      const startLine = doc.lineAt(token.from);
      const lastLine = doc.lineAt(token.to);
      if (revealedLines(state, startLine.from, lastLine.to)) {
        for (let n = startLine.number; n <= lastLine.number; n++) {
          ranges.push(Decoration.line({ class: "cm-lp-math-source" }).range(doc.line(n).from));
        }
        highlightTex(doc.sliceString(token.from, token.to), token.from, ranges);
        continue;
      }
      // A block replacement must cover the trailing line break, or vertical
      // cursor motion skips the following line.
      const to = lastLine.to < doc.length ? lastLine.to + 1 : doc.length;
      ranges.push(
        Decoration.replace({ widget: new KatexWidget(token.latex, true), block: true }).range(startLine.from, to),
      );
    } else if (revealedSpan(state, token.from, token.to)) {
      ranges.push(Decoration.mark({ class: "cm-lp-math-source" }).range(token.from, token.to));
      highlightTex(doc.sliceString(token.from, token.to), token.from, ranges);
    } else {
      ranges.push(Decoration.replace({ widget: new KatexWidget(token.latex, false) }).range(token.from, token.to));
    }
  }
}

/** Decorations plus the math scan they were built from, which a
 * selection-only change reuses. */
interface PreviewState {
  decorations: DecorationSet;
  tokens: MathToken[];
}

function buildPreviewState(state: EditorState, reuseTokens: MathToken[] | null): PreviewState {
  const tree = syntaxTree(state);
  const tokens = reuseTokens ?? findMathTokens(state.doc.toString());
  const ranges: Range<Decoration>[] = [];
  collectFormatting(state, tree, ranges);
  collectMath(state, tree, tokens, ranges);
  return { decorations: Decoration.set(ranges, true), tokens };
}

const decorationField = StateField.define<PreviewState>({
  create: (state) => buildPreviewState(state, null),
  update: (previous, transaction) =>
    revealChanged(transaction)
      ? buildPreviewState(transaction.state, transaction.docChanged ? null : previous.tokens)
      : previous,
  provide: (field) => EditorView.decorations.from(field, (value) => value.decorations),
});

const BOX_BORDER = "1px solid var(--border-variant)";

const livePreviewTheme = EditorView.theme({
  ".cm-math-inline": { cursor: "pointer" },
  // Padding, not margin: CodeMirror measures block widgets by their border box.
  ".cm-math-block": { cursor: "pointer", display: "block", textAlign: "center", padding: "0.4em 0" },
  ".cm-lp-mark": { color: "var(--muted)" },
  ".cm-lp-strong": { fontWeight: "600" },
  ".cm-lp-em": { fontStyle: "italic" },
  ".cm-lp-strike": { textDecoration: "line-through" },
  ".cm-lp-code": {
    fontFamily: "var(--mono)",
    fontSize: "0.875em",
    backgroundColor: "var(--surface)",
    border: BOX_BORDER,
    borderRadius: "4px",
    padding: "0.1em 0.3em",
  },
  ".cm-lp-codeblock, .cm-lp-codefoot": {
    marginLeft: "var(--editor-pad-x)",
    marginRight: "var(--editor-pad-x)",
  },
  ".cm-lp-codeblock": {
    fontFamily: "var(--mono)",
    fontSize: "13px",
    backgroundColor: "var(--surface)",
    borderLeft: BOX_BORDER,
    borderRight: BOX_BORDER,
    padding: "0 0.75rem",
  },
  ".cm-lp-codeblock-top": {
    borderTop: BOX_BORDER,
    borderTopLeftRadius: "8px",
    borderTopRightRadius: "8px",
  },
  ".cm-lp-codeblock-bottom": {
    borderBottom: BOX_BORDER,
    borderBottomLeftRadius: "8px",
    borderBottomRightRadius: "8px",
    paddingBottom: "0.25rem",
  },
  ".cm-lp-codehdr": { paddingTop: "0.125rem", paddingBottom: "0.125rem", borderBottom: BOX_BORDER },
  ".cm-lp-code-lang": { color: "var(--subtext)", fontSize: "0.85em" },
  // A fixed line height absorbs the closing fence's reveal, so the foot never changes height.
  ".cm-lp-codefoot": { fontFamily: "var(--mono)", fontSize: "13px", lineHeight: "0.85rem", padding: "0 0 0 0.75rem" },
  ".cm-lp-fencemark": { color: "var(--muted)", fontSize: "0.85em", verticalAlign: "middle" },
  ".cm-lp-fencemark-hidden": { fontSize: "0" },
  ".cm-lp-fencemark-foot": { color: "var(--muted)", fontSize: "0.85em", opacity: "0" },
  ".cm-lp-fencemark-foot-shown": { opacity: "1" },
  ".cm-lp-math-source": { fontFamily: "var(--mono)", fontSize: "0.9em" },
  ".cm-tex-dollar": { color: "var(--syntax-red)" },
  ".cm-tex-cmd": { color: "var(--syntax-purple)" },
  ".cm-tex-brace": { color: "var(--syntax-text)" },
  ".cm-lp-quote": { borderLeft: "3px solid var(--border)", paddingLeft: "0.75rem", color: "var(--subtext)" },
  ".cm-lp-bullet, .cm-lp-listmark": { color: "var(--subtext)" },
  ".cm-lp-link": {
    color: "var(--accent-blue-strong)",
    textDecoration: "underline",
    textDecorationColor: "var(--link-underline)",
    textUnderlineOffset: "2px",
  },
  // Zero line height keeps the marker from changing the heading's line box.
  ".cm-lp-headermark, .cm-lp-headermark-hidden": { color: "var(--muted)", fontWeight: "400", lineHeight: "0" },
  ".cm-lp-headermark-hidden": { fontSize: "0" },
  ".cm-lp-h1": { fontSize: "1.375em", fontWeight: "600" },
  ".cm-lp-h2": { fontSize: "1.25em", fontWeight: "600" },
  ".cm-lp-h3": { fontSize: "1.125em", fontWeight: "600" },
  ".cm-lp-h4, .cm-lp-h5, .cm-lp-h6": { fontWeight: "600" },
});

/** When Up/Down would jump over a collapsed math block, land on its near
 * line instead, which reveals the source — matching a click into the block. */
function stepIntoBlockMath(view: EditorView, forward: boolean): boolean {
  const { state } = view;
  const cursor = state.selection.main;
  if (!cursor.empty) return false;
  const landing = view.moveVertically(cursor, forward).head;
  if (landing === cursor.head) return false;
  const low = Math.min(cursor.head, landing);
  const high = Math.max(cursor.head, landing);
  const tree = syntaxTree(state);
  for (const token of state.field(decorationField).tokens) {
    if (!token.block || inCode(tree, token.from)) continue;
    const { from } = state.doc.lineAt(token.from);
    const { to } = state.doc.lineAt(token.to);
    const crosses = low <= to && high >= from && (cursor.head < from || cursor.head > to);
    if (!crosses || revealedLines(state, from, to)) continue;
    view.dispatch({ selection: { anchor: forward ? from : to }, scrollIntoView: true });
    return true;
  }
  return false;
}

function enclosingCodeBlock(state: EditorState, pos: number): SyntaxNode | null {
  for (let node: SyntaxNode | null = syntaxTree(state).resolveInner(pos, -1); node; node = node.parent) {
    if (node.name === "FencedCode" || node.name === "CodeBlock") return node;
  }
  return null;
}

/** On the opening line of an unclosed, empty fence, scaffold the closing fence
 * and land on the blank line between. */
function autoCloseFence(view: EditorView): boolean {
  const { state } = view;
  const cursor = state.selection.main;
  if (!cursor.empty) return false;
  const node = enclosingCodeBlock(state, cursor.head);
  if (!node || node.name !== "FencedCode") return false;
  const pos = cursor.head;
  const onOpenLine = state.doc.lineAt(pos).number === state.doc.lineAt(node.from).number;
  const closed = node.getChildren("CodeMark").length > 1;
  if (!onOpenLine || closed || state.sliceDoc(pos, node.to).trim() !== "") return false;
  view.dispatch({
    changes: { from: pos, insert: "\n\n```" },
    selection: { anchor: pos + 1 },
    scrollIntoView: true,
    userEvent: "input",
  });
  return true;
}

/** Enter inside a code block is a newline, not a send. */
function codeBlockEnter(view: EditorView): boolean {
  if (autoCloseFence(view)) return true;
  const { state } = view;
  if (!enclosingCodeBlock(state, state.selection.main.from)) return false;
  view.dispatch({ ...state.replaceSelection("\n"), scrollIntoView: true, userEvent: "input" });
  return true;
}

/** Wrap each range in `mark`, or unwrap it when the mark already hugs it. */
function toggleInlineMark(view: EditorView, mark: string): boolean {
  const { state } = view;
  const len = mark.length;
  const transaction = state.changeByRange((range) => {
    const before = state.sliceDoc(Math.max(0, range.from - len), range.from);
    const after = state.sliceDoc(range.to, Math.min(state.doc.length, range.to + len));
    if (before === mark && after === mark) {
      return {
        changes: [{ from: range.from - len, to: range.from }, { from: range.to, to: range.to + len }],
        range: EditorSelection.range(range.from - len, range.to - len),
      };
    }
    return {
      changes: [{ from: range.from, insert: mark }, { from: range.to, insert: mark }],
      range: EditorSelection.range(range.from + len, range.to + len),
    };
  });
  view.dispatch(state.update(transaction, { scrollIntoView: true, userEvent: "input" }));
  return true;
}

const codeHighlightStyle = HighlightStyle.define([
  { tag: [t.comment, t.lineComment, t.blockComment, t.docString], color: "var(--syntax-comment)", fontStyle: "italic" },
  {
    tag: [t.keyword, t.controlKeyword, t.operatorKeyword, t.definitionKeyword, t.moduleKeyword, t.self],
    color: "var(--syntax-purple)",
  },
  { tag: [t.function(t.variableName), t.function(t.propertyName)], color: "var(--syntax-blue)" },
  { tag: [t.string, t.special(t.string), t.regexp, t.character, t.escape], color: "var(--syntax-green)" },
  { tag: [t.number, t.integer, t.float, t.bool, t.atom, t.null], color: "var(--syntax-orange)" },
  { tag: [t.className, t.typeName, t.namespace], color: "var(--syntax-yellow)" },
  {
    tag: [t.operator, t.derefOperator, t.compareOperator, t.arithmeticOperator, t.logicOperator],
    color: "var(--syntax-cyan)",
  },
  {
    tag: [t.punctuation, t.separator, t.bracket, t.squareBracket, t.paren, t.brace, t.angleBracket],
    color: "var(--syntax-text)",
  },
]);

/** Formatting, KaTeX math, and code highlighting over markdown source. Enter
 * inside a code block breaks the line; Shift+Enter continues a list or quote. */
export const livePreview: Extension = [
  pointerSelection,
  decorationField,
  Prec.highest(keymap.of([
    { key: "Enter", run: codeBlockEnter },
    { key: "Shift-Enter", run: (view) => autoCloseFence(view) || insertNewlineContinueMarkup(view) },
  ])),
  Prec.high(keymap.of([
    { key: "ArrowDown", run: (view) => stepIntoBlockMath(view, true) },
    { key: "ArrowUp", run: (view) => stepIntoBlockMath(view, false) },
    { key: "Mod-b", run: (view) => toggleInlineMark(view, "**") },
    { key: "Mod-i", run: (view) => toggleInlineMark(view, "*") },
  ])),
  syntaxHighlighting(codeHighlightStyle),
  livePreviewTheme,
];
