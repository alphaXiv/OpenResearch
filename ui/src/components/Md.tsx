// Chat markdown with evidence mentions, mirroring openresearch.sh's
// MarkdownContent: `<file path="..." lines="20-40"/>` tags (and plain relative
// links) render as chips that open the file as a right-pane tab, and
// `<run id="..."/>` tags render as chips that open a run's logs — so the agent
// can cite the code and the run behind a claim.

import { Check, Copy, FileCode, ScrollText } from "lucide-react";
import { memo, useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import rehypeKatex from "rehype-katex";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import { resolveSyntaxLanguage } from "../syntaxLanguage";
import { highlight } from "../syntaxHighlight";

// Chat blocks are short; cap tokenizing well below the file viewer's limit.
const HIGHLIGHT_MAX_BYTES = 100_000;

/** A fenced code block: syntax-highlighted body + a copy button. */
function CodeBlock({ code, lang }: { code: string; lang: string | null }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    navigator.clipboard?.writeText(code).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };
  return (
    <div className="md-code [position:relative] [margin:10px_0] [&_pre]:[margin:0] [&:hover_.md-code-copy]:[opacity:1]">
      <button className="md-code-copy [position:absolute] [top:6px] [right:6px] [display:inline-flex] [align-items:center] [justify-content:center] [width:26px] [height:26px] [color:var(--muted)] [background:var(--base)] [border:1px_solid_var(--border-variant)] [border-radius:var(--radius-sm)] [opacity:0] [transition:opacity_0.12s_ease,_color_0.12s_ease] [&:hover]:[color:var(--text)] [&:hover]:[border-color:var(--muted)]" title="Copy" aria-label="Copy code" onClick={copy}>
        {copied ? <Check size={13} /> : <Copy size={13} />}
      </button>
      <pre>
        <code>{highlight(code, lang, HIGHLIGHT_MAX_BYTES)}</code>
      </pre>
    </div>
  );
}

interface MdastNode {
  children?: MdastNode[];
  data?: { hName?: string; hProperties?: Record<string, string> };
  type: string;
  value?: string;
}

/** Pull `name="value"` (or single-quoted) attributes off a tag's attribute run. */
function parseTagAttrs(attrs: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const attr of attrs.matchAll(/([\w-]+)=(["'])(.*?)\2/g)) {
    const key = attr[1];
    if (key) out[key.toLowerCase()] = attr[3] ?? "";
  }
  return out;
}

/** Split raw-html text around `<file .../>` and `<run .../>` tags into text +
 * custom nodes. A `<file>` needs a `path`; a `<run>` needs an `id` — a tag
 * missing its required attribute is almost certainly not ours, so leave it as
 * text. */
function parseMentionHtml(value: string): MdastNode[] | null {
  const regex = /<(file|run)\b([^>]*?)\/?>/gi;
  const nodes: MdastNode[] = [];
  let lastIndex = 0;
  let matched = false;

  for (const match of value.matchAll(regex)) {
    const tag = (match[1] ?? "").toLowerCase();
    const attrs = parseTagAttrs(match[2] ?? "");
    const required = tag === "run" ? "id" : "path";
    if (!attrs[required]) continue;

    matched = true;
    if (match.index > lastIndex) {
      nodes.push({ type: "text", value: value.slice(lastIndex, match.index) });
    }
    nodes.push({
      children: [],
      data: { hName: tag === "run" ? "run-mention" : "file-mention", hProperties: attrs },
      type: tag === "run" ? "runMention" : "fileMention",
    });
    lastIndex = match.index + match[0].length;
  }

  if (!matched) return null;
  if (lastIndex < value.length) {
    nodes.push({ type: "text", value: value.slice(lastIndex) });
  }
  return nodes;
}

function replaceMentions(parent: MdastNode) {
  const children = parent.children;
  if (!children) return;
  for (let i = 0; i < children.length; i += 1) {
    const child = children[i];
    if (!child) continue;
    if (child.type === "html" && typeof child.value === "string") {
      const replacement = parseMentionHtml(child.value);
      if (replacement) {
        children.splice(i, 1, ...replacement);
        i += replacement.length - 1;
        continue;
      }
    }
    replaceMentions(child);
  }
}

function remarkMentions() {
  return (tree: MdastNode) => replaceMentions(tree);
}

function FileChip({
  path,
  lines,
  exp,
  onOpenFile,
}: {
  path: string;
  lines?: string;
  /** Experiment id this file was cited from, if any (`<file exp=…>`). */
  exp?: string;
  onOpenFile?: (path: string, line?: number, exp?: string) => void;
}) {
  const name = path.split("/").pop() || path;
  // `lines` may be a single line or a range ("20-40"); show the first.
  const line = lines ? Number.parseInt(lines, 10) || undefined : undefined;
  const label = line != null ? `${name}:${line}` : name;
  return (
    <button
      className="file-chip"
      title={`Open ${path}`}
      onClick={() => onOpenFile?.(path, line, exp)}
      disabled={!onOpenFile}
    >
      <FileCode size={12} />
      <span className="file-chip-label">{label}</span>
    </button>
  );
}

/** A `<run id="..."/>` evidence mention: a chip that opens the run's logs (the
 * only evidence channel for a metric). `label` overrides the default text so a
 * claim can read "…, not significant [+3.65pp]" rather than a bare run id. */
function RunChip({
  id,
  label,
  onOpenRun,
}: {
  id: string;
  label?: string;
  onOpenRun?: (runId: string) => void;
}) {
  return (
    <button
      className="file-chip run-chip"
      title={`Open logs for run ${id}`}
      onClick={() => onOpenRun?.(id)}
      disabled={!onOpenRun}
    >
      <ScrollText size={12} />
      <span className="file-chip-label">{label || "logs"}</span>
    </button>
  );
}

// Matches regions the math normalizer must not touch: fenced code blocks
// (tolerating an unclosed fence mid-stream) and inline code spans.
const CODE_REGIONS = /(```[\s\S]*?(?:```|$)|~~~[\s\S]*?(?:~~~|$)|`[^`\n]*`)/g;

/** Single-dollar math is off: prose like "cost $8–18 across nodes ($1.45
 * each)" would pair its dollar signs into an inline-math region. Math must
 * use `$$` delimiters — mid-paragraph `$$...$$` still renders inline. */
export const remarkMathOptions = { singleDollarTextMath: false };

/** Rewrite `\(...\)` / `\[...\]` math delimiters to remark-math's `$$` forms
 * (`$` alone is not math — see `remarkMathOptions`).
 *
 * Agents emit LaTeX with backslash delimiters, which plain markdown mangles:
 * `\(` parses as an escaped paren and `_` as emphasis. remark-math only
 * recognizes dollar delimiters, so convert before parsing — skipping code
 * blocks and inline code, where backslashes are literal. */
export function normalizeMathDelimiters(text: string): string {
  if (!text.includes("\\(") && !text.includes("\\[")) return text;
  return text
    .split(CODE_REGIONS)
    .map((seg, i) => {
      if (i % 2 === 1) return seg; // odd segments are code — leave untouched
      return seg
        .replace(/\\\[([\s\S]+?)\\\]/g, (_, inner: string) => `$$${inner}$$`)
        .replace(/\\\(([\s\S]+?)\\\)/g, (_, inner: string) => `$$${inner}$$`);
    })
    .join("");
}

/** A link target that is a file path rather than a web URL. */
function isFileHref(href: string): boolean {
  if (/^[a-z][a-z0-9+.-]*:/i.test(href)) return false; // has a scheme
  if (href.startsWith("#") || href.startsWith("//")) return false;
  return true;
}

/** Shared `code`/`pre` renderers: fenced blocks (language-*) become
 * highlighted CodeBlocks with a copy button; inline code stays a plain
 * <code> chip. The <pre> wrapper is handled inside CodeBlock, so
 * react-markdown's is unwrapped. Reused by the Artifacts markdown renderer. */
export const mdCodeComponents: Record<string, (props: any) => ReactNode> = {
  code: ({ node: _node, className, children, ...rest }: any) => {
    const cls: string = className ?? "";
    const match = /language-(\w+)/.exec(cls);
    const raw = String(children ?? "").replace(/\n$/, "");
    const isBlock = match != null || raw.includes("\n");
    if (!isBlock) {
      return (
        <code className={cls} {...rest}>
          {children}
        </code>
      );
    }
    const lang = match ? resolveSyntaxLanguage(match[1]) : null;
    return <CodeBlock code={raw} lang={lang} />;
  },
  pre: ({ children }: any) => <>{children}</>,
};

/** Memoized: markdown + KaTeX parsing is the expensive part of a chat render,
 * and during streaming only the growing part's text actually changes — every
 * other Md in the transcript can skip the re-parse (memo compares `text` by
 * value; keep `onOpenFile` referentially stable at call sites). */
export const Md = memo(function Md({
  text,
  onOpenFile,
  onOpenRun,
}: {
  text: string;
  onOpenFile?: (path: string, line?: number, exp?: string) => void;
  onOpenRun?: (runId: string) => void;
}) {
  const components: Record<string, (props: any) => ReactNode> = {
    "file-mention": (props) => (
      <FileChip path={props.path} lines={props.lines} exp={props.exp} onOpenFile={onOpenFile} />
    ),
    "run-mention": (props) => (
      <RunChip id={props.id} label={props.label} onOpenRun={onOpenRun} />
    ),
    a: ({ node: _node, href, children, ...rest }) => {
      // Agents sometimes link files as plain markdown links; open those as
      // file tabs instead of navigating the dashboard away.
      if (href && isFileHref(href) && onOpenFile) {
        return <FileChip path={decodeURI(href)} onOpenFile={onOpenFile} />;
      }
      return (
        <a href={href} target="_blank" rel="noopener noreferrer" {...rest}>
          {children}
        </a>
      );
    },
    ...mdCodeComponents,
  };

  return (
    <div className="md [min-width:0] [word-break:break-word] [color:var(--text)] [line-height:1.62] [&_>_*:first-child]:[margin-top:0] [&_>_*:last-child]:[margin-bottom:0] [&_p]:[margin:10px_0] [&_strong]:[color:var(--text)] [&_strong]:[font-weight:var(--fw-semibold)] [&_pre]:[background:var(--surface)] [&_pre]:[border:1px_solid_color-mix(in_oklab,_var(--border)_50%,_transparent)] [&_pre]:[border-radius:var(--radius-md)] [&_pre]:[padding:8px_12px] [&_pre]:[overflow-x:auto] [&_pre]:[font-size:var(--fs-sm)] [&_pre]:[color:var(--text)] [&_code]:[font-family:var(--mono)] [&_code]:[font-size:0.9em] [&_code]:[font-weight:var(--fw-medium)] [&_code]:[color:var(--primary)] [&_code]:[background:var(--panel)] [&_code]:[border:1px_solid_var(--border-variant)] [&_code]:[border-radius:var(--radius-xs)] [&_code]:[padding:1px_5px] [&_.katex]:[font-size:1.05em] [&_.katex-display]:[margin:12px_0] [&_.katex-display]:[overflow-x:auto] [&_.katex-display]:[overflow-y:hidden] [&_.katex-display]:[padding:2px_0] [&_.file-chip]:[display:inline-flex] [&_.file-chip]:[align-items:center] [&_.file-chip]:[gap:4px] [&_.file-chip]:[max-width:100%] [&_.file-chip]:[margin:0_1px] [&_.file-chip]:[padding:0_6px] [&_.file-chip]:[vertical-align:baseline] [&_.file-chip]:[font-family:var(--mono)] [&_.file-chip]:[font-size:0.9em] [&_.file-chip]:[font-weight:var(--fw-medium)] [&_.file-chip]:[color:var(--text)] [&_.file-chip]:[background:var(--panel)] [&_.file-chip]:[border:1px_solid_var(--border-variant)] [&_.file-chip]:[border-radius:var(--radius-xs)] [&_.file-chip]:[cursor:pointer] [&_.file-chip:hover:not(:disabled)]:[background:var(--surface)] [&_.file-chip:hover:not(:disabled)]:[color:var(--primary)] [&_.file-chip_svg]:[flex:none] [&_.file-chip_svg]:[opacity:0.6] [&_.file-chip-label]:[max-width:260px] [&_.file-chip-label]:[overflow:hidden] [&_.file-chip-label]:[text-overflow:ellipsis] [&_.file-chip-label]:[white-space:nowrap] [&_.run-chip_svg]:[opacity:1] [&_.run-chip_svg]:[color:var(--primary)] [&_pre_code]:[background:none] [&_pre_code]:[border:none] [&_pre_code]:[color:inherit] [&_pre_code]:[padding:0] [&_pre_code]:[font-weight:var(--fw-regular)] [&_h1]:[color:var(--text)] [&_h1]:[font-size:1.05em] [&_h1]:[font-weight:var(--fw-semibold)] [&_h1]:[margin:12px_0_6px] [&_h2]:[color:var(--text)] [&_h2]:[font-size:1.05em] [&_h2]:[font-weight:var(--fw-semibold)] [&_h2]:[margin:12px_0_6px] [&_h3]:[color:var(--text)] [&_h3]:[font-size:1.05em] [&_h3]:[font-weight:var(--fw-semibold)] [&_h3]:[margin:12px_0_6px] [&_h4]:[color:var(--text)] [&_h4]:[font-size:1.05em] [&_h4]:[font-weight:var(--fw-semibold)] [&_h4]:[margin:12px_0_6px] [&_ul]:[margin:6px_0] [&_ul]:[padding-left:22px] [&_ol]:[margin:6px_0] [&_ol]:[padding-left:22px] [&_li::marker]:[color:var(--primary)] [&_a]:[color:var(--primary)] [&_table]:[border-collapse:collapse] [&_table]:[display:block] [&_table]:[width:max-content] [&_table]:[max-width:100%] [&_table]:[font-size:var(--fs-md)] [&_table]:[margin:10px_0] [&_table]:[border:1px_solid_var(--border)] [&_table]:[border-radius:var(--radius-md)] [&_table]:[overflow-x:auto] [&_th]:[border-bottom:1px_solid_var(--border-variant)] [&_th]:[padding:8px_14px] [&_th]:[text-align:left] [&_th]:[color:var(--text)] [&_th]:[word-break:normal] [&_th]:[overflow-wrap:break-word] [&_td]:[border-bottom:1px_solid_var(--border-variant)] [&_td]:[padding:8px_14px] [&_td]:[text-align:left] [&_td]:[color:var(--text)] [&_td]:[word-break:normal] [&_td]:[overflow-wrap:break-word] [&_tr:last-child_td]:[border-bottom:none] [&_thead_th]:[background:var(--surface)] [&_thead_th]:[font-weight:var(--fw-medium)] [&_thead_th]:[color:var(--text)] [&_thead_th]:[border-bottom:1px_solid_var(--border)] [&_tbody_tr:hover_td]:[background:var(--surface-bright)] [&_blockquote]:[margin:6px_0] [&_blockquote]:[padding:2px_0_2px_10px] [&_blockquote]:[border-left:3px_solid_var(--border)] [&_blockquote]:[color:var(--subtext)] [:is(&,_.openresearch-diff,_.file-view)_.token.comment]:[font-style:italic] [:is(&,_.openresearch-diff,_.file-view)_.token.prolog]:[font-style:italic] [:is(&,_.openresearch-diff,_.file-view)_.token.cdata]:[font-style:italic] [:is(&,_.openresearch-diff,_.file-view)_.token.operator]:[color:var(--syntax-cyan)] [:is(&,_.openresearch-diff,_.file-view)_.token.entity]:[color:var(--syntax-cyan)] [:is(&,_.openresearch-diff,_.file-view)_.token.url]:[color:var(--syntax-cyan)] [:is(&,_.openresearch-diff,_.file-view)_.token.comment]:[color:var(--syntax-comment)] [:is(&,_.openresearch-diff,_.file-view)_.token.prolog]:[color:var(--syntax-comment)] [:is(&,_.openresearch-diff,_.file-view)_.token.cdata]:[color:var(--syntax-comment)] [:is(&,_.openresearch-diff,_.file-view)_.token.punctuation]:[color:var(--syntax-text)] [:is(&,_.openresearch-diff,_.file-view)_.token.property]:[color:var(--syntax-red)] [:is(&,_.openresearch-diff,_.file-view)_.token.tag]:[color:var(--syntax-red)] [:is(&,_.openresearch-diff,_.file-view)_.token.deleted]:[color:var(--syntax-red)] [:is(&,_.openresearch-diff,_.file-view)_.token.constant]:[color:var(--syntax-orange)] [:is(&,_.openresearch-diff,_.file-view)_.token.symbol]:[color:var(--syntax-orange)] [:is(&,_.openresearch-diff,_.file-view)_.token.boolean]:[color:var(--syntax-orange)] [:is(&,_.openresearch-diff,_.file-view)_.token.number]:[color:var(--syntax-orange)] [:is(&,_.openresearch-diff,_.file-view)_.token.selector]:[color:var(--syntax-green)] [:is(&,_.openresearch-diff,_.file-view)_.token.attr-name]:[color:var(--syntax-green)] [:is(&,_.openresearch-diff,_.file-view)_.token.char]:[color:var(--syntax-green)] [:is(&,_.openresearch-diff,_.file-view)_.token.inserted]:[color:var(--syntax-green)] [:is(&,_.openresearch-diff,_.file-view)_.token.string]:[color:var(--syntax-green)] [:is(&,_.openresearch-diff,_.file-view)_.token.builtin]:[color:var(--syntax-yellow)] [:is(&,_.openresearch-diff,_.file-view)_.token.atrule]:[color:var(--syntax-orange)] [:is(&,_.openresearch-diff,_.file-view)_.token.attr-value]:[color:var(--syntax-orange)] [:is(&,_.openresearch-diff,_.file-view)_.token.keyword]:[color:var(--syntax-purple)] [:is(&,_.openresearch-diff,_.file-view)_.token.function]:[color:var(--syntax-blue)] [:is(&,_.openresearch-diff,_.file-view)_.token.decorator]:[color:var(--syntax-blue)] [:is(&,_.openresearch-diff,_.file-view)_.token.def]:[color:var(--syntax-blue)] [:is(&,_.openresearch-diff,_.file-view)_.token.class-name]:[color:var(--syntax-yellow)] [:is(&,_.openresearch-diff,_.file-view)_.token.namespace]:[color:var(--syntax-yellow)] [:is(&,_.openresearch-diff,_.file-view)_.token.regex]:[color:var(--syntax-green)] [:is(&,_.openresearch-diff,_.file-view)_.token.important]:[color:var(--syntax-red)] [:is(&,_.openresearch-diff,_.file-view)_.token.variable]:[color:var(--syntax-red)] [:is(&,_.openresearch-diff,_.file-view)_.token.parameter]:[color:var(--syntax-text)]">
      <ReactMarkdown
        remarkPlugins={[remarkGfm, [remarkMath, remarkMathOptions], remarkMentions]}
        rehypePlugins={[rehypeKatex]}
        components={components as any}
      >
        {normalizeMathDelimiters(text)}
      </ReactMarkdown>
    </div>
  );
});
