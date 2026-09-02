import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { useLocale } from "../locale";
// Chat markdown with evidence mentions, mirroring openresearch.sh's
// MarkdownContent: `<file path="..." lines="20-40"/>` tags (and plain relative
// links) render as chips that open the file as a right-pane tab, and
// `<run id="..."/>` tags render as chips that open a run's logs — so the agent
// can cite the code and the run behind a claim.

import { Check, Copy, FileCode, PanelRight, ScrollText } from "lucide-react";
import { memo, useMemo, useState, type ReactNode } from "react";
import { Markdown as StreamingMarkdown } from "@clo/react-markdown";
import { defaultUrlTransform } from "react-markdown";
import rehypeKatex from "rehype-katex";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { unified } from "unified";
import { resolveSyntaxLanguage } from "../syntaxLanguage";
import { highlight } from "../syntaxHighlight";
import { normalizeMarkdownForRendering } from "../markdownNormalization";
import { tabOpenGestureHandlers, type TabOpenIntent } from "../tabPreview";
import { IconButton } from "./ui";

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
    <div className="md-code relative my-2.5 mx-0 [&_pre]:m-0 [&:hover_.md-code-copy]:opacity-100">
      <IconButton size="small" className="md-code-copy absolute top-1.5 end-1.5 bg-background opacity-0" title={m.md_copy()} aria-label={m.md_copy_code()} onClick={copy}>
        {copied ? <Check size={13} /> : <Copy size={13} />}
      </IconButton>
      <pre>
        <code>{highlight(code, lang, HIGHLIGHT_MAX_BYTES)}</code>
      </pre>
    </div>
  );
}

interface MdastNode {
  children?: MdastNode[];
  data?: { hName?: string; hProperties?: Record<string, string> };
  position?: MdastPosition;
  type: string;
  value?: string;
}

interface MdastPoint {
  column: number;
  line: number;
  offset?: number;
}

interface MdastPosition {
  end: MdastPoint;
  start: MdastPoint;
}

interface HastNode {
  children?: HastNode[];
  properties?: Record<string, unknown>;
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

function pointAt(value: string, start: MdastPoint, relativeOffset: number): MdastPoint {
  let line = start.line;
  let column = start.column;
  for (let i = 0; i < relativeOffset; i += 1) {
    if (value[i] === "\n") {
      line += 1;
      column = 1;
    } else {
      column += 1;
    }
  }
  return {
    line,
    column,
    ...(start.offset == null ? {} : { offset: start.offset + relativeOffset }),
  };
}

function positionFor(node: MdastNode, start: number, end: number): MdastPosition | undefined {
  if (!node.position || typeof node.value !== "string") return undefined;
  return {
    start: pointAt(node.value, node.position.start, start),
    end: pointAt(node.value, node.position.start, end),
  };
}

/** Split raw HTML around valid `<file>` and `<run>` tags into custom nodes. */
function parseMentionHtml(node: MdastNode): MdastNode[] | null {
  const value = node.value ?? "";
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
      nodes.push({
        type: "text",
        value: value.slice(lastIndex, match.index),
        position: positionFor(node, lastIndex, match.index),
      });
    }
    const matchEnd = match.index + match[0].length;
    nodes.push({
      children: [],
      data: { hName: tag === "run" ? "run-mention" : "file-mention", hProperties: attrs },
      position: positionFor(node, match.index, matchEnd),
      type: tag === "run" ? "runMention" : "fileMention",
    });
    lastIndex = matchEnd;
  }

  if (!matched) return null;
  if (lastIndex < value.length) {
    nodes.push({
      type: "text",
      value: value.slice(lastIndex),
      position: positionFor(node, lastIndex, value.length),
    });
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
      const replacement = parseMentionHtml(child);
      if (replacement) {
        children.splice(i, 1, ...replacement);
        i += replacement.length - 1;
        continue;
      }
    }
    if (child.type === "inlineCode" && typeof child.value === "string") {
      const replacement = parseMentionHtml({ ...child, position: undefined });
      if (replacement?.length === 1 && replacement[0]?.type.endsWith("Mention")) {
        children.splice(i, 1, replacement[0]);
        continue;
      }
    }
    replaceMentions(child);
  }
}

function remarkMentions() {
  return (tree: MdastNode) => replaceMentions(tree);
}

function rehypeSafeUrls() {
  return (tree: HastNode) => {
    const visit = (node: HastNode) => {
      for (const key of ["href", "src"]) {
        if (node.properties && Object.hasOwn(node.properties, key)) {
          node.properties[key] = defaultUrlTransform(String(node.properties[key] || ""));
        }
      }
      node.children?.forEach(visit);
    };
    visit(tree);
  };
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
  onOpenFile?: (
    path: string,
    line: number | undefined,
    exp: string | undefined,
    ref: string | undefined,
    intent: TabOpenIntent,
  ) => void;
}) {
  const name = path.split("/").pop() || path;
  // `lines` may be a single line or a range ("20-40"); show the first.
  const line = lines ? Number.parseInt(lines, 10) || undefined : undefined;
  const label = line != null ? `${name}:${line}` : name;
  return (
    <button
      className="file-chip"
      title={onOpenFile ? m.a11y_open_file_in_panel({ path: ltr(path) }) : path}
      {...tabOpenGestureHandlers<HTMLButtonElement>((intent) =>
        onOpenFile?.(path, line, exp, undefined, intent),
      )}
      disabled={!onOpenFile}
    >
      <FileCode size={12} />
      <span className="file-chip-label">{label}</span>
      <PanelRight className="file-chip-open" size={12} aria-hidden="true" />
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
  onOpenRun?: (runId: string, intent: TabOpenIntent) => void;
}) {
  return (
    <button
      className="file-chip run-chip"
      title={onOpenRun ? m.a11y_open_run_in_panel({ id: ltr(id) }) : m.a11y_run_id({ id: ltr(id) })}
      {...tabOpenGestureHandlers<HTMLButtonElement>((intent) => onOpenRun?.(id, intent))}
      disabled={!onOpenRun}
    >
      <ScrollText size={12} />
      <span className="file-chip-label">{label || m.tree_view_logs()}</span>
      <PanelRight className="file-chip-open" size={12} aria-hidden="true" />
    </button>
  );
}

export const remarkMathOptions = { singleDollarTextMath: true };

const markdownProcessor = unified()
  .use(remarkParse)
  .use(remarkGfm)
  .use(remarkMath, remarkMathOptions)
  .use(remarkMentions)
  .use(remarkRehype)
  .use(rehypeSafeUrls)
  .use(rehypeKatex);

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
  resolveFilePath,
  resolveImageSrc,
  predict = false,
}: {
  text: string;
  onOpenFile?: (
    path: string,
    line: number | undefined,
    exp: string | undefined,
    ref: string | undefined,
    intent: TabOpenIntent,
  ) => void;
  onOpenRun?: (runId: string, intent: TabOpenIntent) => void;
  resolveFilePath?: (path: string) => string | null;
  resolveImageSrc?: (src: string) => string | null;
  predict?: boolean;
}) {
  useLocale();
  const components: Record<string, (props: any) => ReactNode> = useMemo(() => ({
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
        let decoded: string;
        try {
          decoded = decodeURI(href);
        } catch {
          return <span>{children}</span>;
        }
        const path = resolveFilePath ? resolveFilePath(decoded) : decoded;
        return path ? <FileChip path={path} onOpenFile={onOpenFile} /> : <span>{children}</span>;
      }
      return (
        <a href={href} target="_blank" rel="noopener noreferrer" {...rest}>
          {children}
        </a>
      );
    },
    th: ({ node: _node, ...rest }) => <th dir="auto" {...rest} />,
    td: ({ node: _node, ...rest }) => <td dir="auto" {...rest} />,
    img: ({ node: _node, src, alt, className, ...rest }) => {
      if (!src || typeof src !== "string") return null;
      const resolved = resolveImageSrc ? resolveImageSrc(src) : src;
      if (!resolved) return null;
      return (
        <img
          {...rest}
          src={resolved}
          alt={alt ?? ""}
          loading="lazy"
          className={`block max-w-full h-auto my-3 rounded-sm border border-border ${className ?? ""}`}
       />
      );
    },
    ...mdCodeComponents,
  }), [onOpenFile, onOpenRun, resolveFilePath, resolveImageSrc]);

  return (
    <div dir="auto" data-streaming={predict || undefined} className="md min-w-0 wrap-anywhere text-text leading-[1.62] [&_>_*:first-child]:mt-0 [&_>_*:last-child]:mb-0 [&_p]:my-2.5 [&_p]:mx-0 [&_strong]:text-text [&_strong]:font-semibold [&_pre]:bg-surface [&_pre]:border [&_pre]:border-border-muted [&_pre]:rounded-md [&_pre]:py-2 [&_pre]:px-3 [&_pre]:overflow-x-auto [&_pre]:text-sm [&_pre]:text-text [&_code]:font-mono [&_code]:text-sm [&_code]:font-medium [&_code]:text-primary [&_code]:bg-panel [&_code]:border [&_code]:border-border-variant [&_code]:rounded-xs [&_code]:py-px [&_code]:px-[5px] [&_.katex]:text-prose-emphasis [&_.katex-display]:my-3 [&_.katex-display]:mx-0 [&_.katex-display]:overflow-x-auto [&_.katex-display]:overflow-y-hidden [&_.katex-display]:py-0.5 [&_.katex-display]:px-0 [&_.file-chip]:inline-flex [&_.file-chip]:items-center [&_.file-chip]:gap-1 [&_.file-chip]:max-w-full [&_.file-chip]:my-0 [&_.file-chip]:mx-px [&_.file-chip]:py-0 [&_.file-chip]:px-1.5 [&_.file-chip]:align-baseline [&_.file-chip]:font-mono [&_.file-chip]:text-sm [&_.file-chip]:font-medium [&_.file-chip]:text-text [&_.file-chip]:bg-panel [&_.file-chip]:border [&_.file-chip]:border-border-variant [&_.file-chip]:rounded-xs [&_.file-chip]:cursor-pointer [&_.file-chip:hover:not(:disabled)]:bg-surface [&_.file-chip:hover:not(:disabled)]:text-primary [&_.file-chip_svg]:flex-none [&_.file-chip_svg]:opacity-60 [&_.file-chip-label]:max-w-65 [&_.file-chip-label]:overflow-hidden [&_.file-chip-label]:text-ellipsis [&_.file-chip-label]:whitespace-nowrap [&_.run-chip_svg]:opacity-100 [&_.run-chip_svg]:text-primary [&_pre_code]:bg-none [&_pre_code]:bg-transparent [&_pre_code]:border-0 [&_pre_code]:text-inherit [&_pre_code]:p-0 [&_pre_code]:font-normal [&_h1]:text-text [&_h1]:text-prose-emphasis [&_h1]:font-semibold [&_h1]:mt-3 [&_h1]:mx-0 [&_h1]:mb-1.5 [&_h2]:text-text [&_h2]:text-prose-emphasis [&_h2]:font-semibold [&_h2]:mt-3 [&_h2]:mx-0 [&_h2]:mb-1.5 [&_h3]:text-text [&_h3]:text-prose-emphasis [&_h3]:font-semibold [&_h3]:mt-3 [&_h3]:mx-0 [&_h3]:mb-1.5 [&_h4]:text-text [&_h4]:text-prose-emphasis [&_h4]:font-semibold [&_h4]:mt-3 [&_h4]:mx-0 [&_h4]:mb-1.5 [&_ul]:my-1.5 [&_ul]:mx-0 [&_ul]:ps-5.5 [&_ol]:my-1.5 [&_ol]:mx-0 [&_ol]:ps-5.5 [&_li::marker]:text-primary [&_a]:text-primary [&_table]:border-collapse [&_table]:block [&_table]:w-max [&_table]:max-w-full [&_table]:text-sm [&_table]:my-2.5 [&_table]:mx-0 [&_table]:border [&_table]:border-border [&_table]:rounded-md [&_table]:overflow-x-auto [&_th]:border-b [&_th]:border-b-border-variant [&_th]:py-2 [&_th]:px-3.5 [&_th]:text-start [&_th]:text-text [&_th]:break-normal [&_th]:break-words [&_td]:border-b [&_td]:border-b-border-variant [&_td]:py-2 [&_td]:px-3.5 [&_td]:text-start [&_td]:text-text [&_td]:break-normal [&_td]:break-words [&_tr:last-child_td]:border-b-0 [&_thead_th]:bg-surface [&_thead_th]:font-medium [&_thead_th]:text-text [&_thead_th]:border-b [&_thead_th]:border-b-border [&_tbody_tr:hover_td]:bg-surface-bright [&_blockquote]:my-1.5 [&_blockquote]:mx-0 [&_blockquote]:pt-0.5 [&_blockquote]:pe-0 [&_blockquote]:pb-0.5 [&_blockquote]:ps-2.5 [&_blockquote]:border-s-[3px] [&_blockquote]:border-s-border [&_blockquote]:text-subtext [:is(&,_.openresearch-diff,_.file-view)_.token.comment]:italic [:is(&,_.openresearch-diff,_.file-view)_.token.prolog]:italic [:is(&,_.openresearch-diff,_.file-view)_.token.cdata]:italic [:is(&,_.openresearch-diff,_.file-view)_.token.operator]:text-syntax-cyan [:is(&,_.openresearch-diff,_.file-view)_.token.entity]:text-syntax-cyan [:is(&,_.openresearch-diff,_.file-view)_.token.url]:text-syntax-cyan [:is(&,_.openresearch-diff,_.file-view)_.token.comment]:text-syntax-comment [:is(&,_.openresearch-diff,_.file-view)_.token.prolog]:text-syntax-comment [:is(&,_.openresearch-diff,_.file-view)_.token.cdata]:text-syntax-comment [:is(&,_.openresearch-diff,_.file-view)_.token.punctuation]:text-syntax-text [:is(&,_.openresearch-diff,_.file-view)_.token.property]:text-syntax-red [:is(&,_.openresearch-diff,_.file-view)_.token.tag]:text-syntax-red [:is(&,_.openresearch-diff,_.file-view)_.token.deleted]:text-syntax-red [:is(&,_.openresearch-diff,_.file-view)_.token.constant]:text-syntax-orange [:is(&,_.openresearch-diff,_.file-view)_.token.symbol]:text-syntax-orange [:is(&,_.openresearch-diff,_.file-view)_.token.boolean]:text-syntax-orange [:is(&,_.openresearch-diff,_.file-view)_.token.number]:text-syntax-orange [:is(&,_.openresearch-diff,_.file-view)_.token.selector]:text-syntax-green [:is(&,_.openresearch-diff,_.file-view)_.token.attr-name]:text-syntax-green [:is(&,_.openresearch-diff,_.file-view)_.token.char]:text-syntax-green [:is(&,_.openresearch-diff,_.file-view)_.token.inserted]:text-syntax-green [:is(&,_.openresearch-diff,_.file-view)_.token.string]:text-syntax-green [:is(&,_.openresearch-diff,_.file-view)_.token.builtin]:text-syntax-yellow [:is(&,_.openresearch-diff,_.file-view)_.token.atrule]:text-syntax-orange [:is(&,_.openresearch-diff,_.file-view)_.token.attr-value]:text-syntax-orange [:is(&,_.openresearch-diff,_.file-view)_.token.keyword]:text-syntax-purple [:is(&,_.openresearch-diff,_.file-view)_.token.function]:text-syntax-blue [:is(&,_.openresearch-diff,_.file-view)_.token.decorator]:text-syntax-blue [:is(&,_.openresearch-diff,_.file-view)_.token.def]:text-syntax-blue [:is(&,_.openresearch-diff,_.file-view)_.token.class-name]:text-syntax-yellow [:is(&,_.openresearch-diff,_.file-view)_.token.namespace]:text-syntax-yellow [:is(&,_.openresearch-diff,_.file-view)_.token.regex]:text-syntax-green [:is(&,_.openresearch-diff,_.file-view)_.token.important]:text-syntax-red [:is(&,_.openresearch-diff,_.file-view)_.token.variable]:text-syntax-red [:is(&,_.openresearch-diff,_.file-view)_.token.parameter]:text-syntax-text">
      <StreamingMarkdown
        content={normalizeMarkdownForRendering(text, { predictMath: predict })}
        processor={markdownProcessor}
        components={components}
        predict={predict}
     />
    </div>
  );
});
