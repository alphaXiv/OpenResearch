import {
  Check,
  ChevronRight,
  Copy,
  ExternalLink,
  Code,
  FileText,
  MousePointerClick,
  Package,
  Settings2,
  Trash2,
} from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import rehypeKatex from "rehype-katex";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import {
  artifactUrl,
  deleteArtifact,
  fmtBytes,
  type ArtifactEntry,
  type Project,
  type ProjectArtifacts,
} from "../api";
import { CodeView } from "./CodeView";
import { FileTypeIcon, isImageFile, isMarkdownFile } from "./FileTypeIcon";
import { mdCodeComponents, normalizeMathDelimiters, remarkMathOptions } from "./Md";
import { ICON_BUTTON_BASE_CLASS_NAME, ICON_BUTTON_CLASS_NAME, SETTINGS_LOADING_CLASS_NAME, SPINNER_CLASS_NAME } from "../styleClasses";

const TOOLTIP_ICON_BUTTON_CLASS_NAME = `${ICON_BUTTON_CLASS_NAME} tip-up [&[data-tip]::after]:[top:auto] [&[data-tip]::after]:[bottom:calc(100%_+_6px)]`;

/** Any href with a URI scheme (https:, mailto:, data:, …) or a
 * protocol-relative // — i.e. not an artifact-relative path to resolve. */
function isExternalSrc(src: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(src) || src.startsWith("//");
}

/** Resolve a Markdown target within the artifacts root. URL suffixes stay
 * outside the encoded filesystem path, and upward escapes are rejected. */
function artifactTargetUrl(projectId: string, folder: string, src: string): string | null {
  const hashAt = src.indexOf("#");
  const beforeHash = hashAt === -1 ? src : src.slice(0, hashAt);
  const hash = hashAt === -1 ? "" : src.slice(hashAt);
  const queryAt = beforeHash.indexOf("?");
  const pathname = queryAt === -1 ? beforeHash : beforeHash.slice(0, queryAt);
  const query = queryAt === -1 ? "" : beforeHash.slice(queryAt + 1);
  const parts = pathname.startsWith("/")
    ? []
    : folder.split("/").filter((part) => part.length > 0);

  for (const part of pathname.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (parts.length === 0) return null;
      parts.pop();
    } else {
      parts.push(part);
    }
  }

  const path = parts.join("/");
  if (!path) return null;
  const queryParams = new URLSearchParams(query);
  queryParams.delete("path");
  const querySuffix = queryParams.toString();
  return `${artifactUrl(projectId, path)}${querySuffix ? `&${querySuffix}` : ""}${hash}`;
}

/** Drop a leading YAML frontmatter block so it doesn't render as markdown. */
function stripFrontmatter(md: string): string {
  if (!md.startsWith("---")) return md;
  const end = md.indexOf("\n---", 3);
  return end === -1 ? md : md.slice(end + 4).replace(/^\r?\n/, "");
}

/** Raw text preview cap — matches the repo file viewer's truncation cap. */
const MAX_TEXT_PREVIEW = 512 * 1024;

/** Tree pane width: draggable divider, persisted across reloads. */
const TREE_WIDTH_KEY = "orx:files-tree-width";
const COLLAPSED_DIRS_KEY_PREFIX = "orx:artifacts-collapsed:";
const TREE_MIN_WIDTH = 180;
const TREE_MAX_WIDTH = 560;
const TREE_DEFAULT_WIDTH = 280;

function initialTreeWidth(): number {
  try {
    const w = Number(localStorage.getItem(TREE_WIDTH_KEY));
    if (Number.isFinite(w) && w >= TREE_MIN_WIDTH && w <= TREE_MAX_WIDTH) return w;
  } catch {
    // storage unavailable — fall through to the default
  }
  return TREE_DEFAULT_WIDTH;
}

function initialCollapsed(projectId: string): Set<string> {
  try {
    const raw = localStorage.getItem(`${COLLAPSED_DIRS_KEY_PREFIX}${projectId}`);
    if (!raw) return new Set();
    const value: unknown = JSON.parse(raw);
    if (!Array.isArray(value)) return new Set();
    return new Set(value.filter((path): path is string => typeof path === "string"));
  } catch {
    return new Set();
  }
}

/** Depth-first lookup of a tree entry by its directory-relative path. */
function findEntry(entries: ArtifactEntry[], path: string): ArtifactEntry | null {
  for (const e of entries) {
    if (e.path === path) return e;
    if (e.isDir && path.startsWith(e.path + "/")) {
      const hit = findEntry(e.children ?? [], path);
      if (hit) return hit;
    }
  }
  return null;
}

/** Artifact markdown with relative image/link paths rewritten to the raw
 * artifact endpoint, scoped to the markdown file's parent directory. */
export function ArtifactMarkdown({
  projectId,
  folder,
  markdown,
}: {
  projectId: string;
  folder: string;
  markdown: string;
}) {
  const resolve = (src: string) => {
    if (isExternalSrc(src)) return src;
    return artifactTargetUrl(projectId, folder, src);
  };
  return (
    <div className="md [min-width:0] [word-break:break-word] [color:var(--text)] [line-height:1.62] [&_>_*:first-child]:[margin-top:0] [&_>_*:last-child]:[margin-bottom:0] [&_p]:[margin:10px_0] [&_strong]:[color:var(--text)] [&_strong]:[font-weight:var(--fw-semibold)] [&_pre]:[background:var(--surface)] [&_pre]:[border:1px_solid_color-mix(in_oklab,_var(--border)_50%,_transparent)] [&_pre]:[border-radius:var(--radius-md)] [&_pre]:[padding:8px_12px] [&_pre]:[overflow-x:auto] [&_pre]:[font-size:var(--fs-sm)] [&_pre]:[color:var(--text)] [&_code]:[font-family:var(--mono)] [&_code]:[font-size:0.9em] [&_code]:[font-weight:var(--fw-medium)] [&_code]:[color:var(--primary)] [&_code]:[background:var(--panel)] [&_code]:[border:1px_solid_var(--border-variant)] [&_code]:[border-radius:var(--radius-xs)] [&_code]:[padding:1px_5px] [&_.katex]:[font-size:1.05em] [&_.katex-display]:[margin:12px_0] [&_.katex-display]:[overflow-x:auto] [&_.katex-display]:[overflow-y:hidden] [&_.katex-display]:[padding:2px_0] [&_.file-chip]:[display:inline-flex] [&_.file-chip]:[align-items:center] [&_.file-chip]:[gap:4px] [&_.file-chip]:[max-width:100%] [&_.file-chip]:[margin:0_1px] [&_.file-chip]:[padding:0_6px] [&_.file-chip]:[vertical-align:baseline] [&_.file-chip]:[font-family:var(--mono)] [&_.file-chip]:[font-size:0.9em] [&_.file-chip]:[font-weight:var(--fw-medium)] [&_.file-chip]:[color:var(--text)] [&_.file-chip]:[background:var(--panel)] [&_.file-chip]:[border:1px_solid_var(--border-variant)] [&_.file-chip]:[border-radius:var(--radius-xs)] [&_.file-chip]:[cursor:pointer] [&_.file-chip:hover:not(:disabled)]:[background:var(--surface)] [&_.file-chip:hover:not(:disabled)]:[color:var(--primary)] [&_.file-chip_svg]:[flex:none] [&_.file-chip_svg]:[opacity:0.6] [&_.file-chip-label]:[max-width:260px] [&_.file-chip-label]:[overflow:hidden] [&_.file-chip-label]:[text-overflow:ellipsis] [&_.file-chip-label]:[white-space:nowrap] [&_.run-chip_svg]:[opacity:1] [&_.run-chip_svg]:[color:var(--primary)] [&_pre_code]:[background:none] [&_pre_code]:[border:none] [&_pre_code]:[color:inherit] [&_pre_code]:[padding:0] [&_pre_code]:[font-weight:var(--fw-regular)] [&_h1]:[color:var(--text)] [&_h1]:[font-weight:var(--fw-semibold)] [&_h2]:[color:var(--text)] [&_h2]:[font-weight:var(--fw-semibold)] [&_h3]:[color:var(--text)] [&_h3]:[font-weight:var(--fw-semibold)] [&_h4]:[color:var(--text)] [&_h4]:[font-weight:var(--fw-semibold)] [&_ul]:[margin:6px_0] [&_ul]:[padding-left:22px] [&_ol]:[margin:6px_0] [&_ol]:[padding-left:22px] [&_li::marker]:[color:var(--primary)] [&_a]:[color:var(--primary)] [&_table]:[border-collapse:collapse] [&_table]:[font-size:var(--fs-md)] [&_table]:[margin:10px_0] [&_table]:[border:1px_solid_var(--border)] [&_table]:[border-radius:var(--radius-md)] [&_th]:[border-bottom:1px_solid_var(--border-variant)] [&_th]:[padding:8px_14px] [&_th]:[text-align:left] [&_th]:[color:var(--text)] [&_th]:[word-break:normal] [&_th]:[overflow-wrap:break-word] [&_td]:[border-bottom:1px_solid_var(--border-variant)] [&_td]:[padding:8px_14px] [&_td]:[text-align:left] [&_td]:[color:var(--text)] [&_td]:[word-break:normal] [&_td]:[overflow-wrap:break-word] [&_tr:last-child_td]:[border-bottom:none] [&_thead_th]:[background:var(--surface)] [&_thead_th]:[font-weight:var(--fw-medium)] [&_thead_th]:[color:var(--text)] [&_thead_th]:[border-bottom:1px_solid_var(--border)] [&_tbody_tr:hover_td]:[background:var(--surface-bright)] [&_blockquote]:[margin:6px_0] [&_blockquote]:[padding:2px_0_2px_10px] [&_blockquote]:[border-left:3px_solid_var(--border)] [&_blockquote]:[color:var(--subtext)] [:is(&,_.openresearch-diff,_.file-view)_.token.comment]:[font-style:italic] [:is(&,_.openresearch-diff,_.file-view)_.token.prolog]:[font-style:italic] [:is(&,_.openresearch-diff,_.file-view)_.token.cdata]:[font-style:italic] [:is(&,_.openresearch-diff,_.file-view)_.token.operator]:[color:var(--syntax-cyan)] [:is(&,_.openresearch-diff,_.file-view)_.token.entity]:[color:var(--syntax-cyan)] [:is(&,_.openresearch-diff,_.file-view)_.token.url]:[color:var(--syntax-cyan)] [:is(&,_.openresearch-diff,_.file-view)_.token.comment]:[color:var(--syntax-comment)] [:is(&,_.openresearch-diff,_.file-view)_.token.prolog]:[color:var(--syntax-comment)] [:is(&,_.openresearch-diff,_.file-view)_.token.cdata]:[color:var(--syntax-comment)] [:is(&,_.openresearch-diff,_.file-view)_.token.punctuation]:[color:var(--syntax-text)] [:is(&,_.openresearch-diff,_.file-view)_.token.property]:[color:var(--syntax-red)] [:is(&,_.openresearch-diff,_.file-view)_.token.tag]:[color:var(--syntax-red)] [:is(&,_.openresearch-diff,_.file-view)_.token.deleted]:[color:var(--syntax-red)] [:is(&,_.openresearch-diff,_.file-view)_.token.constant]:[color:var(--syntax-orange)] [:is(&,_.openresearch-diff,_.file-view)_.token.symbol]:[color:var(--syntax-orange)] [:is(&,_.openresearch-diff,_.file-view)_.token.boolean]:[color:var(--syntax-orange)] [:is(&,_.openresearch-diff,_.file-view)_.token.number]:[color:var(--syntax-orange)] [:is(&,_.openresearch-diff,_.file-view)_.token.selector]:[color:var(--syntax-green)] [:is(&,_.openresearch-diff,_.file-view)_.token.attr-name]:[color:var(--syntax-green)] [:is(&,_.openresearch-diff,_.file-view)_.token.char]:[color:var(--syntax-green)] [:is(&,_.openresearch-diff,_.file-view)_.token.inserted]:[color:var(--syntax-green)] [:is(&,_.openresearch-diff,_.file-view)_.token.string]:[color:var(--syntax-green)] [:is(&,_.openresearch-diff,_.file-view)_.token.builtin]:[color:var(--syntax-yellow)] [:is(&,_.openresearch-diff,_.file-view)_.token.atrule]:[color:var(--syntax-orange)] [:is(&,_.openresearch-diff,_.file-view)_.token.attr-value]:[color:var(--syntax-orange)] [:is(&,_.openresearch-diff,_.file-view)_.token.keyword]:[color:var(--syntax-purple)] [:is(&,_.openresearch-diff,_.file-view)_.token.function]:[color:var(--syntax-blue)] [:is(&,_.openresearch-diff,_.file-view)_.token.decorator]:[color:var(--syntax-blue)] [:is(&,_.openresearch-diff,_.file-view)_.token.def]:[color:var(--syntax-blue)] [:is(&,_.openresearch-diff,_.file-view)_.token.class-name]:[color:var(--syntax-yellow)] [:is(&,_.openresearch-diff,_.file-view)_.token.namespace]:[color:var(--syntax-yellow)] [:is(&,_.openresearch-diff,_.file-view)_.token.regex]:[color:var(--syntax-green)] [:is(&,_.openresearch-diff,_.file-view)_.token.important]:[color:var(--syntax-red)] [:is(&,_.openresearch-diff,_.file-view)_.token.variable]:[color:var(--syntax-red)] [:is(&,_.openresearch-diff,_.file-view)_.token.parameter]:[color:var(--syntax-text)] artifact-md [font-size:var(--fs-lg)] [&_h1]:[font-size:2em] [&_h1]:[line-height:1.18] [&_h1]:[margin:28px_0_14px] [&_h2]:[font-size:1.5em] [&_h2]:[line-height:1.25] [&_h2]:[margin:28px_0_10px] [&_h3]:[font-size:1.2em] [&_h3]:[line-height:1.35] [&_h3]:[margin:22px_0_8px] [&_h4]:[font-size:1em] [&_h4]:[line-height:1.4] [&_h4]:[margin:18px_0_6px] [&_table]:[display:block] [&_table]:[width:max-content] [&_table]:[max-width:100%] [&_table]:[overflow-x:auto] [&_.artifact-img]:[display:block] [&_.artifact-img]:[margin:12px_0] [&_.artifact-img_img]:[max-width:100%] [&_.artifact-img_img]:[height:auto] [&_.artifact-img_img]:[border:1px_solid_var(--border)] [&_.artifact-img_img]:[border-radius:var(--radius-sm)] [&_.artifact-img-caption]:[display:block] [&_.artifact-img-caption]:[margin-top:4px] [&_.artifact-img-caption]:[text-align:center] [&_.artifact-img-caption]:[font-size:var(--fs-sm)] [&_.artifact-img-caption]:[color:var(--subtext)]">
      <ReactMarkdown
        remarkPlugins={[remarkGfm, [remarkMath, remarkMathOptions]]}
        rehypePlugins={[rehypeKatex]}
        components={{
          // In-page anchors (headings, GFM footnotes) keep their hash href
          // and stay in the page; everything else resolves + opens a tab.
          a: ({ href, children, ...rest }) => {
            const isHash = !href || href.startsWith("#");
            const resolved = isHash ? href : resolve(href);
            if (!resolved) return <span>{children}</span>;
            return (
              <a
                {...rest}
                href={resolved}
                {...(isHash ? {} : { target: "_blank", rel: "noopener noreferrer" })}
              >
                {children}
              </a>
            );
          },
          img: ({ src, alt }) => {
            if (!src || typeof src !== "string") return null;
            const url = resolve(src);
            if (!url) return null;
            return (
              <a href={url} target="_blank" rel="noopener noreferrer" className="artifact-img">
                <img src={url} alt={alt ?? ""} loading="lazy" />
                {alt && <span className="artifact-img-caption">{alt}</span>}
              </a>
            );
          },
          ...mdCodeComponents,
        }}
      >
        {normalizeMathDelimiters(stripFrontmatter(markdown))}
      </ReactMarkdown>
    </div>
  );
}

type PreviewKind = "markdown" | "image" | "pdf" | "text";

function previewKind(entry: ArtifactEntry): PreviewKind {
  if (isMarkdownFile(entry.name)) return "markdown";
  if (isImageFile(entry.name)) return "image";
  if (/\.pdf$/i.test(entry.name)) return "pdf";
  return "text";
}

/** Fetched body for kinds that need text: markdown or raw text.
 * `binary` flags NUL bytes so we don't dump garbage into a <pre>. */
function useTextBody(projectId: string, entry: ArtifactEntry, kind: PreviewKind) {
  const [text, setText] = useState<string | null>(null);
  const [binary, setBinary] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const wantsText = kind === "markdown" || (kind === "text" && entry.size <= MAX_TEXT_PREVIEW);

  useEffect(() => {
    // Reset before the wantsText guard: a refire on the same mounted entry
    // (modifiedAt changed — file rewritten on disk) must not leave the
    // previous body or binary/error flags behind.
    setText(null);
    setBinary(false);
    setError(null);
    if (!wantsText) return;
    let cancelled = false;
    const load = fetch(artifactUrl(projectId, entry.path)).then((r) => {
      if (!r.ok) throw new Error(`Failed to load artifact (${r.status})`);
      return r.text();
    });
    load
      .then((body) => {
        if (cancelled) return;
        if (body.includes("\u0000")) setBinary(true);
        else setText(body);
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, entry.path, entry.modifiedAt, kind, wantsText]);

  return { text, binary, error, wantsText };
}

/** Right pane: the selected artifact rendered inline — markdown as a document,
 * images/PDFs directly, and everything else as code. */
function PreviewPane({
  projectId,
  entry,
  onDelete,
}: {
  projectId: string;
  entry: ArtifactEntry;
  onDelete: (path: string) => void;
}) {
  const kind = previewKind(entry);
  const { text, binary, error, wantsText } = useTextBody(projectId, entry, kind);
  const [showSource, setShowSource] = useState(false);
  const isDoc = kind === "markdown";
  const mdFolder = entry.path.split("/").slice(0, -1).join("/");
  const rawUrl = artifactUrl(projectId, entry.path);

  let body: ReactNode;
  if (kind === "image") {
    body = (
      <a className="fpreview-image [display:flex] [align-items:flex-start] [justify-content:center] [padding:24px] [&_img]:[max-width:100%] [&_img]:[height:auto] [&_img]:[border:1px_solid_var(--border)] [&_img]:[border-radius:var(--radius-sm)]" href={rawUrl} target="_blank" rel="noopener noreferrer">
        <img src={rawUrl} alt={entry.name} />
      </a>
    );
  } else if (kind === "pdf") {
    body = <iframe className="fpreview-pdf [display:block] [width:100%] [height:100%] [border:0]" title={entry.name} src={rawUrl} />;
  } else if (!wantsText || binary) {
    body = (
      <div className="file-view-note [padding:10px_16px] [font-size:var(--fs-sm)] [color:var(--muted)]">
        {binary ? "Binary file — no inline preview." : "File too large to preview inline."}{" "}
        <a href={rawUrl} target="_blank" rel="noopener noreferrer">
          Open raw
        </a>
      </div>
    );
  } else if (error) {
    body = <div className="file-view-note [padding:10px_16px] [font-size:var(--fs-sm)] [color:var(--muted)]">Failed to load: {error}</div>;
  } else if (text === null) {
    body = (
      <div className={SETTINGS_LOADING_CLASS_NAME}>
        <span className={SPINNER_CLASS_NAME} /> Loading…
      </div>
    );
  } else if (isDoc && !showSource) {
    body = <ArtifactMarkdown projectId={projectId} folder={mdFolder} markdown={text} />;
  } else {
    body = <CodeView text={text} path={entry.path} />;
  }

  return (
    // `file-view` scopes the shared syntax-token colors onto the code view.
    <div className="fpreview [flex:1] [min-width:0] [background:var(--base)] file-view [display:flex] [flex-direction:column] [height:100%] [min-height:0]">
      <div className="fpreview-head [height:40px] [display:flex] [align-items:center] [gap:8px] [padding:0_14px] [border-bottom:1px_solid_var(--border-variant)] [color:var(--subtext)] [flex-shrink:0]">
        <FileText size={13} style={{ flexShrink: 0 }} />
        <code className="fpreview-path [font-family:var(--mono)] [font-size:var(--fs-sm)] [color:var(--text)] [flex:1] [min-width:0] [overflow:hidden] [text-overflow:ellipsis] [white-space:nowrap]" title={entry.path}>
          {entry.path}
        </code>
        <span className="fpreview-date [font-size:var(--fs-xs)] [color:var(--muted)] [white-space:nowrap] [flex-shrink:0]">
          Modified{" "}
          {new Date(entry.modifiedAt).toLocaleString(undefined, {
            dateStyle: "medium",
            timeStyle: "short",
          })}
        </span>
        {kind === "text" && (
          <span className="fpreview-size [font-size:var(--fs-xs)] [color:var(--muted)] [white-space:nowrap] [flex-shrink:0]">{fmtBytes(entry.size)}</span>
        )}
        {isDoc && (
          <button
            className={`${ICON_BUTTON_CLASS_NAME} ${showSource ? "active" : ""}`}
            data-tip={showSource ? "Rendered view" : "View source"}
            data-tip-align="end"
            aria-label={showSource ? "Rendered view" : "View source"}
            onClick={() => setShowSource((s) => !s)}
          >
            <Code size={13} />
          </button>
        )}
        <a
          className={ICON_BUTTON_CLASS_NAME}
          href={rawUrl}
          target="_blank"
          rel="noopener noreferrer"
          data-tip="Open raw in new tab"
          data-tip-align="end"
          aria-label="Open raw in new tab"
        >
          <ExternalLink size={13} />
        </a>
        <button
          className={ICON_BUTTON_CLASS_NAME}
          data-tip="Delete artifact"
          data-tip-align="end"
          aria-label="Delete artifact"
          onClick={() => {
            if (window.confirm(`Delete "${entry.path}" from the artifacts directory?`))
              onDelete(entry.path);
          }}
        >
          <Trash2 size={13} />
        </button>
      </div>
      <div className={`fpreview-body [flex:1] [min-height:0] [overflow:auto] [&.doc]:[padding:18px_28px_48px] [&.doc_.artifact-md]:[max-width:var(--readable-col)] [&.doc_.artifact-md]:[margin:0_auto] ${isDoc && !showSource ? "doc" : ""}`}>{body}</div>
    </div>
  );
}

function TreeRows({
  entries,
  depth,
  collapsed,
  selected,
  onToggle,
  onSelect,
  onDelete,
}: {
  entries: ArtifactEntry[];
  depth: number;
  collapsed: Set<string>;
  selected: string | null;
  onToggle: (path: string) => void;
  onSelect: (path: string) => void;
  onDelete: (path: string) => void;
}) {
  return (
    <>
      {entries.map((e) => {
        const indent = { paddingLeft: 8 + depth * 14 };
        if (e.isDir) {
          const open = !collapsed.has(e.path);
          return (
            <div key={e.path}>
              <div className="file-tree-row [display:flex] [align-items:center] [gap:6px] [width:100%] [padding:3px_10px] [border:none] [background:transparent] [color:var(--text)] [text-align:left] [cursor:pointer] [font-family:inherit] [font-size:inherit] [&:hover]:[background:var(--panel)] [&_>_svg]:[flex-shrink:0] [&_>_svg]:[color:var(--subtext)] [&_>_svg.file-tree-chevron]:[color:var(--muted)] artifact-tree-row [&.selected]:[background:var(--panel)] [&.selected:hover]:[background:var(--panel)] [&:hover_.ft-row-delete]:[opacity:1]" style={indent} onClick={() => onToggle(e.path)}>
                <button
                  className="file-tree-chevron [color:var(--muted)] [flex-shrink:0] [button&]:[display:inline-flex] [button&]:[align-items:center] [button&]:[justify-content:center] [button&]:[width:13px] [button&]:[height:13px] [button&]:[padding:0] [button&]:[border:0] [button&]:[background:transparent] [button&_>_svg]:[transition:transform_0.12s_ease] [button&_>_svg.open]:[transform:rotate(90deg)]"
                  aria-label={open ? `Collapse ${e.name}` : `Expand ${e.name}`}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    onToggle(e.path);
                  }}
                >
                  <ChevronRight size={13} className={open ? "open" : ""} />
                </button>
                <span className="file-tree-name [flex:1] [min-width:0] [overflow:hidden] [text-overflow:ellipsis] [white-space:nowrap]">{e.name}</span>
                <button
                  className={`${ICON_BUTTON_BASE_CLASS_NAME} ft-row-delete [width:18px] [height:18px] [opacity:0.35] [&:focus-visible]:[opacity:1]`}
                  data-tip="Delete folder"
                  data-tip-align="end"
                  aria-label={`Delete folder ${e.name}`}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    if (window.confirm(`Delete "${e.path}" from the artifacts directory?`))
                      onDelete(e.path);
                  }}
                >
                  <Trash2 size={12} />
                </button>
              </div>
              {open && (e.children?.length ?? 0) > 0 && (
                <TreeRows
                  entries={e.children ?? []}
                  depth={depth + 1}
                  collapsed={collapsed}
                  selected={selected}
                  onToggle={onToggle}
                  onSelect={onSelect}
                  onDelete={onDelete}
                />
              )}
            </div>
          );
        }

        return (
          <button
            key={e.path}
            type="button"
            className={`file-tree-row [display:flex] [align-items:center] [gap:6px] [width:100%] [padding:3px_10px] [border:none] [background:transparent] [color:var(--text)] [text-align:left] [cursor:pointer] [font-family:inherit] [font-size:inherit] [&:hover]:[background:var(--panel)] [&_>_svg]:[flex-shrink:0] [&_>_svg]:[color:var(--subtext)] [&_>_svg.file-tree-chevron]:[color:var(--muted)] artifact-tree-row [&.selected]:[background:var(--panel)] [&.selected:hover]:[background:var(--panel)] [&:hover_.ft-row-delete]:[opacity:1] ${selected === e.path ? "selected" : ""}`}
            style={indent}
            title={e.path}
            aria-pressed={selected === e.path}
            onClick={() => onSelect(e.path)}
          >
            <FileTypeIcon name={e.name} />
            <span className="file-tree-name [flex:1] [min-width:0] [overflow:hidden] [text-overflow:ellipsis] [white-space:nowrap]">{e.name}</span>
          </button>
        );
      })}
    </>
  );
}

/** The artifacts directory path, copyable in the tree footer. */
function DirFooter({ dir, onOpenStorage }: { dir: string; onOpenStorage: () => void }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="ftree-footer [flex-shrink:0] [display:flex] [align-items:center] [gap:2px] [padding:5px_8px] [border-top:1px_solid_var(--border-variant)] [&_code]:[flex:1] [&_code]:[min-width:0] [&_code]:[direction:rtl] [&_code]:[text-align:left] [&_code]:[font-family:var(--mono)] [&_code]:[font-size:var(--fs-xs)] [&_code]:[color:var(--muted)] [&_code]:[overflow:hidden] [&_code]:[text-overflow:ellipsis] [&_code]:[white-space:nowrap] [&_.icon-btn]:[width:22px] [&_.icon-btn]:[height:22px]" title={dir}>
      <code>{dir}</code>
      <button
        className={TOOLTIP_ICON_BUTTON_CLASS_NAME}
        data-tip={copied ? "Copied!" : "Copy path"}
        aria-label="Copy artifacts directory path"
        onClick={() => {
          void navigator.clipboard?.writeText(dir);
          setCopied(true);
          setTimeout(() => setCopied(false), 1200);
        }}
      >
        {copied ? <Check size={12} /> : <Copy size={12} />}
      </button>
      <button
        className={TOOLTIP_ICON_BUTTON_CLASS_NAME}
        data-tip="Storage settings"
        data-tip-align="end"
        aria-label="Storage settings"
        onClick={onOpenStorage}
      >
        <Settings2 size={12} />
      </button>
    </div>
  );
}

/** Middle-pane Artifacts tab — a split explorer over the project's durable outputs
 * on disk. Tree on the left; the selected entry renders inline on the right
 * (markdown as documents, images and PDFs directly, code as highlighted source). */
export function ArtifactsTab({
  project,
  artifacts,
  onChanged,
  onOpenStorage,
}: {
  project: Project;
  artifacts: ProjectArtifacts | null;
  onChanged: () => void;
  /** Navigate to Settings → Storage (where the data dir can be changed). */
  onOpenStorage: () => void;
}) {
  const [selected, setSelected] = useState<string | null>(null);
  // Folders are open by default — including ones that appear later — so this
  // tracks what the user closed instead.
  const [collapsed, setCollapsed] = useState<Set<string>>(() => initialCollapsed(project.id));
  const [treeWidth, setTreeWidth] = useState(initialTreeWidth);
  const treeRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    try {
      localStorage.setItem(
        `${COLLAPSED_DIRS_KEY_PREFIX}${project.id}`,
        JSON.stringify([...collapsed]),
      );
    } catch {
      // best-effort persistence
    }
  }, [project.id, collapsed]);

  // Drag the divider to resize the tree pane; width persists across reloads.
  // Mirrors App's right-panel resizer: capture the pointer so views under the
  // cursor don't steal the drag, and suppress text selection while dragging.
  const resizeTree = (e: React.PointerEvent) => {
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    const left = treeRef.current?.getBoundingClientRect().left ?? 0;
    const prevUserSelect = document.body.style.userSelect;
    document.body.style.userSelect = "none";
    const onMove = (ev: PointerEvent) => {
      const w = Math.round(ev.clientX - left);
      const clamped = Math.min(Math.max(w, TREE_MIN_WIDTH), TREE_MAX_WIDTH);
      setTreeWidth(clamped);
      try {
        localStorage.setItem(TREE_WIDTH_KEY, String(clamped));
      } catch {
        // best-effort persistence
      }
    };
    const stop = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
      document.body.style.userSelect = prevUserSelect;
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", stop);
    window.addEventListener("pointercancel", stop);
  };

  // Clear a selection that vanished or became a directory on disk.
  useEffect(() => {
    if (!selected || !artifacts) return;
    const entry = findEntry(artifacts.entries, selected);
    if (!entry || entry.isDir) setSelected(null);
  }, [selected, artifacts]);

  const toggle = (path: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const remove = (path: string) => {
    if (selected === path || selected?.startsWith(path + "/")) setSelected(null);
    void deleteArtifact(project.id, path)
      .catch(() => {})
      .finally(onChanged);
  };

  if (!artifacts) {
    return (
      <div className="files-tab [height:100%] [min-height:0] [display:flex] [background:var(--base)]">
        <div className={SETTINGS_LOADING_CLASS_NAME} style={{ padding: 20 }}>
          <span className={SPINNER_CLASS_NAME} /> Loading artifacts…
        </div>
      </div>
    );
  }

  const tree = (entries: ArtifactEntry[]) => (
    <TreeRows
      entries={entries}
      depth={0}
      collapsed={collapsed}
      selected={selected}
      onToggle={toggle}
      onSelect={setSelected}
      onDelete={remove}
    />
  );
  const selectedEntry = selected ? findEntry(artifacts.entries, selected) : null;

  if (artifacts.entries.length === 0) {
    return (
      <div className="files-tab [height:100%] [min-height:0] [display:flex] [background:var(--base)]">
        <div className="files-empty-state [flex:1] [display:flex] [flex-direction:column] [align-items:center] [justify-content:center] [gap:6px] [padding:24px] [text-align:center] [color:var(--muted)] [&_h3]:[margin:6px_0_0] [&_h3]:[font-size:var(--fs-base)] [&_h3]:[font-weight:var(--fw-semibold)] [&_h3]:[color:var(--text)] [&_p]:[margin:0] [&_p]:[max-width:420px] [&_p]:[font-size:var(--fs-md)] [&_p]:[line-height:1.55] [&_p]:[color:var(--subtext)] [&_.ftree-footer]:[margin-top:10px] [&_.ftree-footer]:[max-width:100%] [&_.ftree-footer]:[border:1px_solid_var(--border)] [&_.ftree-footer]:[border-radius:var(--radius-md)] [&_.ftree-footer]:[padding:6px_10px] [&_.ftree-footer]:[background:var(--base)] [&_.ftree-footer_code]:[max-width:380px]">
          <Package size={28} strokeWidth={1.5} />
          <h3>No artifacts yet</h3>
          <p>
            This is the project's durable output space for reports, figures, images, CSVs, PDFs,
            and other research artifacts. Ask the agent for a write-up or add your own files:
          </p>
          <DirFooter dir={artifacts.dir} onOpenStorage={onOpenStorage} />
        </div>
      </div>
    );
  }

  return (
    <div className="files-tab [height:100%] [min-height:0] [display:flex] [background:var(--base)]">
      <div className="ftree-pane [position:relative] [flex-shrink:0] [display:flex] [flex-direction:column] [min-height:0] [border-left:1px_solid_var(--border-variant)] [border-right:1px_solid_var(--border-variant)] [background:var(--base)]" ref={treeRef} style={{ width: treeWidth }}>
        <div className="ftree-resizer [position:absolute] [right:-3px] [top:0] [bottom:0] [width:6px] [cursor:col-resize] [z-index:30] [&:hover]:[background:color-mix(in_oklab,_var(--text)_12%,_transparent)] [&:active]:[background:color-mix(in_oklab,_var(--text)_12%,_transparent)]" onPointerDown={resizeTree} />
        <div className="ftree-scroll [flex:1] [min-height:0] [overflow-y:auto] file-tree [padding:6px_0] [font-size:var(--fs-md)]">
          {tree(artifacts.entries)}
          {artifacts.truncated && (
            <p className="files-truncated [margin:0] [padding:8px_14px] [font-size:var(--fs-xs)] [color:var(--muted)]">Listing truncated — the folder has more artifacts.</p>
          )}
        </div>
        <DirFooter dir={artifacts.dir} onOpenStorage={onOpenStorage} />
      </div>
      {selectedEntry ? (
        // Keyed by path so per-file view state (source toggle, fetched body)
        // starts fresh on every selection instead of leaking across artifacts.
        <PreviewPane
          key={selectedEntry.path}
          projectId={project.id}
          entry={selectedEntry}
          onDelete={remove}
        />
      ) : (
        <div className="fpreview [flex:1] [min-width:0] [display:flex] [flex-direction:column] [min-height:0] [background:var(--base)] fpreview-none [align-items:center] [justify-content:center] [gap:8px] [font-size:var(--fs-md)] [color:var(--muted)]">
          <MousePointerClick size={22} strokeWidth={1.5} />
          <span>Click an artifact to view it</span>
        </div>
      )}
    </div>
  );
}
