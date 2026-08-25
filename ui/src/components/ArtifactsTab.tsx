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
  FILE_PREVIEW_BYTES,
  fmtBytes,
  getArtifactFileText,
  type ArtifactEntry,
  type Project,
  type ProjectArtifacts,
} from "../api";
import { CodeView } from "./CodeView";
import { FileTypeIcon, isMarkdownFile } from "./FileTypeIcon";
import { MediaPreview, mediaPreviewKind, type MediaPreviewKind } from "./MediaPreview";
import { normalizeMarkdownForRendering } from "../markdownNormalization";
import { mdCodeComponents, remarkMathOptions } from "./Md";
import { ICON_BUTTON_BASE_CLASS_NAME, ICON_BUTTON_CLASS_NAME, SETTINGS_LOADING_CLASS_NAME, SPINNER_CLASS_NAME } from "../styleClasses";

const TOOLTIP_ICON_BUTTON_CLASS_NAME = `${ICON_BUTTON_CLASS_NAME} tip-up [&[data-tip]::after]:top-auto [&[data-tip]::after]:bottom-[calc(100%_+_6px)]`;

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

/** Tree pane width: draggable divider, persisted across reloads. */
const TREE_WIDTH_KEY = "orx:files-tree-width";
const COLLAPSED_DIRS_KEY_PREFIX = "orx:artifacts-collapsed:";
const TREE_MIN_WIDTH = 180;
const TREE_MAX_WIDTH = 560;
const TREE_MAX_INDENT_DEPTH = 8;
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
    <div className="md min-w-0 wrap-anywhere text-text leading-[1.62] [&_>_*:first-child]:mt-0 [&_>_*:last-child]:mb-0 [&_p]:my-2.5 [&_p]:mx-0 [&_strong]:text-text [&_strong]:font-semibold [&_pre]:bg-surface [&_pre]:border [&_pre]:border-[color-mix(in_oklab,_var(--border)_50%,_transparent)] [&_pre]:rounded-md [&_pre]:py-2 [&_pre]:px-3 [&_pre]:overflow-x-auto [&_pre]:text-sm [&_pre]:text-text [&_code]:font-mono [&_code]:text-[0.9em] [&_code]:font-medium [&_code]:text-primary [&_code]:bg-panel [&_code]:border [&_code]:border-border-variant [&_code]:rounded-xs [&_code]:py-px [&_code]:px-[5px] [&_.katex]:text-[1.05em] [&_.katex-display]:my-3 [&_.katex-display]:mx-0 [&_.katex-display]:overflow-x-auto [&_.katex-display]:overflow-y-hidden [&_.katex-display]:py-0.5 [&_.katex-display]:px-0 [&_.file-chip]:inline-flex [&_.file-chip]:items-center [&_.file-chip]:gap-1 [&_.file-chip]:max-w-full [&_.file-chip]:my-0 [&_.file-chip]:mx-px [&_.file-chip]:py-0 [&_.file-chip]:px-1.5 [&_.file-chip]:align-baseline [&_.file-chip]:font-mono [&_.file-chip]:text-[0.9em] [&_.file-chip]:font-medium [&_.file-chip]:text-text [&_.file-chip]:bg-panel [&_.file-chip]:border [&_.file-chip]:border-border-variant [&_.file-chip]:rounded-xs [&_.file-chip]:cursor-pointer [&_.file-chip:hover:not(:disabled)]:bg-surface [&_.file-chip:hover:not(:disabled)]:text-primary [&_.file-chip_svg]:flex-none [&_.file-chip_svg]:opacity-60 [&_.file-chip-label]:max-w-65 [&_.file-chip-label]:overflow-hidden [&_.file-chip-label]:text-ellipsis [&_.file-chip-label]:whitespace-nowrap [&_.run-chip_svg]:opacity-100 [&_.run-chip_svg]:text-primary [&_pre_code]:bg-none [&_pre_code]:bg-transparent [&_pre_code]:border-0 [&_pre_code]:text-inherit [&_pre_code]:p-0 [&_pre_code]:font-normal [&_h1]:text-text [&_h1]:font-semibold [&_h2]:text-text [&_h2]:font-semibold [&_h3]:text-text [&_h3]:font-semibold [&_h4]:text-text [&_h4]:font-semibold [&_ul]:my-1.5 [&_ul]:mx-0 [&_ul]:pl-5.5 [&_ol]:my-1.5 [&_ol]:mx-0 [&_ol]:pl-5.5 [&_li::marker]:text-primary [&_a]:text-primary [&_table]:border-collapse [&_table]:text-md [&_table]:my-2.5 [&_table]:mx-0 [&_table]:border [&_table]:border-border [&_table]:rounded-md [&_th]:border-b [&_th]:border-b-border-variant [&_th]:py-2 [&_th]:px-3.5 [&_th]:text-left [&_th]:text-text [&_th]:break-normal [&_th]:break-words [&_td]:border-b [&_td]:border-b-border-variant [&_td]:py-2 [&_td]:px-3.5 [&_td]:text-left [&_td]:text-text [&_td]:break-normal [&_td]:break-words [&_tr:last-child_td]:border-b-0 [&_thead_th]:bg-surface [&_thead_th]:font-medium [&_thead_th]:text-text [&_thead_th]:border-b [&_thead_th]:border-b-border [&_tbody_tr:hover_td]:bg-surface-bright [&_blockquote]:my-1.5 [&_blockquote]:mx-0 [&_blockquote]:pt-0.5 [&_blockquote]:pr-0 [&_blockquote]:pb-0.5 [&_blockquote]:pl-2.5 [&_blockquote]:border-l-[3px] [&_blockquote]:border-l-border [&_blockquote]:text-subtext [:is(&,_.openresearch-diff,_.file-view)_.token.comment]:italic [:is(&,_.openresearch-diff,_.file-view)_.token.prolog]:italic [:is(&,_.openresearch-diff,_.file-view)_.token.cdata]:italic [:is(&,_.openresearch-diff,_.file-view)_.token.operator]:text-syntax-cyan [:is(&,_.openresearch-diff,_.file-view)_.token.entity]:text-syntax-cyan [:is(&,_.openresearch-diff,_.file-view)_.token.url]:text-syntax-cyan [:is(&,_.openresearch-diff,_.file-view)_.token.comment]:text-syntax-comment [:is(&,_.openresearch-diff,_.file-view)_.token.prolog]:text-syntax-comment [:is(&,_.openresearch-diff,_.file-view)_.token.cdata]:text-syntax-comment [:is(&,_.openresearch-diff,_.file-view)_.token.punctuation]:text-syntax-text [:is(&,_.openresearch-diff,_.file-view)_.token.property]:text-syntax-red [:is(&,_.openresearch-diff,_.file-view)_.token.tag]:text-syntax-red [:is(&,_.openresearch-diff,_.file-view)_.token.deleted]:text-syntax-red [:is(&,_.openresearch-diff,_.file-view)_.token.constant]:text-syntax-orange [:is(&,_.openresearch-diff,_.file-view)_.token.symbol]:text-syntax-orange [:is(&,_.openresearch-diff,_.file-view)_.token.boolean]:text-syntax-orange [:is(&,_.openresearch-diff,_.file-view)_.token.number]:text-syntax-orange [:is(&,_.openresearch-diff,_.file-view)_.token.selector]:text-syntax-green [:is(&,_.openresearch-diff,_.file-view)_.token.attr-name]:text-syntax-green [:is(&,_.openresearch-diff,_.file-view)_.token.char]:text-syntax-green [:is(&,_.openresearch-diff,_.file-view)_.token.inserted]:text-syntax-green [:is(&,_.openresearch-diff,_.file-view)_.token.string]:text-syntax-green [:is(&,_.openresearch-diff,_.file-view)_.token.builtin]:text-syntax-yellow [:is(&,_.openresearch-diff,_.file-view)_.token.atrule]:text-syntax-orange [:is(&,_.openresearch-diff,_.file-view)_.token.attr-value]:text-syntax-orange [:is(&,_.openresearch-diff,_.file-view)_.token.keyword]:text-syntax-purple [:is(&,_.openresearch-diff,_.file-view)_.token.function]:text-syntax-blue [:is(&,_.openresearch-diff,_.file-view)_.token.decorator]:text-syntax-blue [:is(&,_.openresearch-diff,_.file-view)_.token.def]:text-syntax-blue [:is(&,_.openresearch-diff,_.file-view)_.token.class-name]:text-syntax-yellow [:is(&,_.openresearch-diff,_.file-view)_.token.namespace]:text-syntax-yellow [:is(&,_.openresearch-diff,_.file-view)_.token.regex]:text-syntax-green [:is(&,_.openresearch-diff,_.file-view)_.token.important]:text-syntax-red [:is(&,_.openresearch-diff,_.file-view)_.token.variable]:text-syntax-red [:is(&,_.openresearch-diff,_.file-view)_.token.parameter]:text-syntax-text artifact-md text-lg [&_h1]:text-[2em] [&_h1]:leading-[1.18] [&_h1]:mt-7 [&_h1]:mx-0 [&_h1]:mb-3.5 [&_h2]:text-[1.5em] [&_h2]:leading-tight [&_h2]:mt-7 [&_h2]:mx-0 [&_h2]:mb-2.5 [&_h3]:text-[1.2em] [&_h3]:leading-[1.35] [&_h3]:mt-5.5 [&_h3]:mx-0 [&_h3]:mb-2 [&_h4]:text-[1em] [&_h4]:leading-[1.4] [&_h4]:mt-4.5 [&_h4]:mx-0 [&_h4]:mb-1.5 [&_table]:block [&_table]:w-max [&_table]:max-w-full [&_table]:overflow-x-auto [&_.artifact-img]:block [&_.artifact-img]:my-3 [&_.artifact-img]:mx-0 [&_.artifact-img_img]:max-w-full [&_.artifact-img_img]:h-auto [&_.artifact-img_img]:border [&_.artifact-img_img]:border-border [&_.artifact-img_img]:rounded-sm [&_.artifact-img-caption]:block [&_.artifact-img-caption]:mt-1 [&_.artifact-img-caption]:text-center [&_.artifact-img-caption]:text-sm [&_.artifact-img-caption]:text-subtext">
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
        {normalizeMarkdownForRendering(stripFrontmatter(markdown))}
      </ReactMarkdown>
    </div>
  );
}

type PreviewKind = "markdown" | MediaPreviewKind | "text" | "download";

function previewKind(entry: ArtifactEntry): PreviewKind {
  if (entry.presentation === "text" && isMarkdownFile(entry.name)) return "markdown";
  return mediaPreviewKind(entry.presentation) ??
    (entry.presentation === "text" || entry.presentation === "unknown" ? "text" : "download");
}

/** Fetched body for kinds that need text: markdown or raw text. */
function useTextBody(projectId: string, entry: ArtifactEntry, kind: PreviewKind) {
  const [text, setText] = useState<string | null>(null);
  const [binary, setBinary] = useState(false);
  const [truncated, setTruncated] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestSequence = useRef(0);
  const hasText = useRef(false);

  const wantsText = kind === "markdown" || (kind === "text" && entry.size <= FILE_PREVIEW_BYTES);

  useEffect(() => {
    // Keep the previous body visible while a same-path rewrite is refetched.
    // The preview component remounts when the selected path changes.
    setBinary(false);
    setTruncated(false);
    setError(null);
    if (!wantsText) return;
    let cancelled = false;
    const request = ++requestSequence.current;
    const load = getArtifactFileText(projectId, entry.path).then((body) => {
      if (!body) throw new Error("Artifact not found");
      return body;
    });
    load
      .then((body) => {
        if (cancelled || request !== requestSequence.current) return;
        if (body.binary) setBinary(true);
        else {
          hasText.current = true;
          setText(body.content);
        }
        setTruncated(body.truncated);
      })
      .catch((e) => {
        if (!cancelled && request === requestSequence.current && !hasText.current) {
          setError(e instanceof Error ? e.message : String(e));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, entry.path, entry.modifiedAt, kind, wantsText]);

  return { text, binary, truncated, error, wantsText };
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
  const { text, binary, truncated, error, wantsText } = useTextBody(projectId, entry, kind);
  const [showSource, setShowSource] = useState(false);
  const isDoc = kind === "markdown";
  const mdFolder = entry.path.split("/").slice(0, -1).join("/");
  const rawUrl = `${artifactUrl(projectId, entry.path)}&v=${entry.modifiedAt}`;

  let body: ReactNode;
  if (kind === "image" || kind === "audio" || kind === "video" || kind === "pdf") {
    body = <MediaPreview kind={kind} url={rawUrl} name={entry.name} />;
  } else if (kind === "download" || !wantsText || binary) {
    body = (
      <div className="file-view-note py-2.5 px-4 text-sm text-muted">
        {kind === "download" || binary
          ? "Binary or unsupported file — no inline preview."
          : "File too large to preview inline."}{" "}
        <a
          href={rawUrl}
          {...(kind === "download" || binary
            ? { download: entry.name }
            : { target: "_blank", rel: "noopener noreferrer" })}
        >
          {kind === "download" || binary ? "Download" : "Open raw"}
        </a>
      </div>
    );
  } else if (error) {
    body = <div className="file-view-note py-2.5 px-4 text-sm text-muted">Failed to load: {error}</div>;
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
    <div className="fpreview flex-1 min-w-0 bg-background file-view flex flex-col h-full min-h-0">
      <div className="fpreview-head h-10 flex items-center gap-2 py-0 px-3.5 border-b border-b-border-variant text-subtext shrink-0">
        <FileText size={13} style={{ flexShrink: 0 }} />
        <code className="fpreview-path font-mono text-sm text-text flex-1 min-w-0 overflow-hidden text-ellipsis whitespace-nowrap" title={entry.path}>
          {entry.path}
        </code>
        <span className="fpreview-date text-xs text-muted whitespace-nowrap shrink-0">
          Modified{" "}
          {new Date(entry.modifiedAt).toLocaleString(undefined, {
            dateStyle: "medium",
            timeStyle: "short",
          })}
        </span>
        {(kind === "text" || kind === "download") && (
          <span className="fpreview-size text-xs text-muted whitespace-nowrap shrink-0">{fmtBytes(entry.size)}</span>
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
      <div className={`fpreview-body flex-1 min-h-0 overflow-auto [&.doc]:pt-4.5 [&.doc]:px-7 [&.doc]:pb-12 [&.doc_.artifact-md]:max-w-readable [&.doc_.artifact-md]:my-0 [&.doc_.artifact-md]:mx-auto ${isDoc && !showSource ? "doc" : ""}`}>
        {body}
        {truncated && (
          <div className="file-view-note py-2.5 px-4 text-sm text-muted">
            File truncated — showing the first 512 KB.
          </div>
        )}
      </div>
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
  onOpenFile,
  onDelete,
}: {
  entries: ArtifactEntry[];
  depth: number;
  collapsed: Set<string>;
  selected: string | null;
  onToggle: (path: string) => void;
  onSelect: (path: string) => void;
  onOpenFile: (path: string) => void;
  onDelete: (path: string) => void;
}) {
  return (
    <div className="flex w-full max-w-full min-w-0 flex-col items-stretch">
      {entries.map((e) => {
        const indent = { paddingLeft: 8 + Math.min(depth, TREE_MAX_INDENT_DEPTH) * 14 };
        if (e.isDir) {
          const open = !collapsed.has(e.path);
          return (
            <div key={e.path} className="min-w-0 max-w-full">
              <div className="file-tree-row flex w-full min-w-0 items-center gap-1.5 py-[3px] px-2.5 border-0 bg-transparent text-text text-left cursor-pointer font-[inherit] text-[length:inherit] [&:hover]:bg-panel [&_>_svg]:shrink-0 [&_>_svg]:text-subtext [&_>_svg.file-tree-chevron]:text-muted artifact-tree-row [&.selected]:bg-panel [&.selected:hover]:bg-panel [&:hover_.ft-row-delete]:opacity-100" style={indent} onClick={() => onToggle(e.path)}>
                <button
                  className="file-tree-chevron text-muted shrink-0 [button&]:inline-flex [button&]:items-center [button&]:justify-center [button&]:w-[13px] [button&]:h-[13px] [button&]:p-0 [button&]:border-0 [button&]:bg-transparent [button&_>_svg]:transition-transform [button&_>_svg]:duration-120 [button&_>_svg]:ease-standard [button&_>_svg.open]:rotate-90"
                  aria-label={open ? `Collapse ${e.name}` : `Expand ${e.name}`}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    onToggle(e.path);
                  }}
                >
                  <ChevronRight size={13} className={open ? "open" : ""} />
                </button>
                <span className="file-tree-name flex-1 min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">{e.name}</span>
                <button
                  className={`${ICON_BUTTON_BASE_CLASS_NAME} ft-row-delete w-4.5 h-4.5 opacity-35 [&:focus-visible]:opacity-100`}
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
                  onOpenFile={onOpenFile}
                  onDelete={onDelete}
                />
              )}
            </div>
          );
        }

        // Artifacts keeps preview in this split view; explicit opens use file tabs.
        return (
          <button
            key={e.path}
            type="button"
            className={`file-tree-row flex w-full min-w-0 items-center gap-1.5 py-[3px] px-2.5 border-0 bg-transparent text-text text-left cursor-pointer font-[inherit] text-[length:inherit] [&:hover]:bg-panel [&_>_svg]:shrink-0 [&_>_svg]:text-subtext [&_>_svg.file-tree-chevron]:text-muted artifact-tree-row [&.selected]:bg-panel [&.selected:hover]:bg-panel [&:hover_.ft-row-delete]:opacity-100 ${selected === e.path ? "selected" : ""}`}
            style={indent}
            title={`${e.path} — Space previews inline; double-click or Enter keeps open in a tab`}
            aria-keyshortcuts="Space Enter"
            aria-pressed={selected === e.path}
            onClick={() => onSelect(e.path)}
            onDoubleClick={() => onOpenFile(e.path)}
            onAuxClick={(event) => {
              if (event.button !== 1) return;
              event.preventDefault();
              onSelect(e.path);
              onOpenFile(e.path);
            }}
            onKeyDown={(event) => {
              if (event.key === " ") {
                event.preventDefault();
                event.stopPropagation();
                onSelect(e.path);
                return;
              }
              if (event.key !== "Enter") return;
              event.preventDefault();
              event.stopPropagation();
              onSelect(e.path);
              onOpenFile(e.path);
            }}
          >
            <FileTypeIcon name={e.name} />
            <span className="file-tree-name flex-1 min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">{e.name}</span>
          </button>
        );
      })}
    </div>
  );
}

/** The artifacts directory path, copyable in the tree footer. */
function DirFooter({ dir, onOpenStorage }: { dir: string; onOpenStorage: () => void }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="ftree-footer shrink-0 flex items-center gap-0.5 py-[5px] px-2 border-t border-t-border-variant [&_code]:flex-1 [&_code]:min-w-0 [&_code]:[direction:rtl] [&_code]:text-left [&_code]:font-mono [&_code]:text-xs [&_code]:text-muted [&_code]:overflow-hidden [&_code]:text-ellipsis [&_code]:whitespace-nowrap [&_.icon-btn]:w-5.5 [&_.icon-btn]:h-5.5" title={dir}>
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
  onOpenFile,
  onOpenStorage,
}: {
  project: Project;
  artifacts: ProjectArtifacts | null;
  onChanged: () => void;
  onOpenFile: (path: string) => void;
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
      <div className="files-tab h-full min-h-0 flex bg-background">
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
      onOpenFile={onOpenFile}
      onDelete={remove}
    />
  );
  const selectedEntry = selected ? findEntry(artifacts.entries, selected) : null;

  if (artifacts.entries.length === 0) {
    return (
      <div className="files-tab h-full min-h-0 flex bg-background">
        <div className="files-empty-state flex-1 flex flex-col items-center justify-center gap-1.5 p-6 text-center text-muted [&_h3]:mt-1.5 [&_h3]:mx-0 [&_h3]:mb-0 [&_h3]:text-base [&_h3]:font-semibold [&_h3]:text-text [&_p]:m-0 [&_p]:max-w-105 [&_p]:text-md [&_p]:leading-[1.55] [&_p]:text-subtext [&_.ftree-footer]:mt-2.5 [&_.ftree-footer]:max-w-full [&_.ftree-footer]:border [&_.ftree-footer]:border-border [&_.ftree-footer]:rounded-md [&_.ftree-footer]:py-1.5 [&_.ftree-footer]:px-2.5 [&_.ftree-footer]:bg-background [&_.ftree-footer_code]:max-w-95">
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
    <div className="files-tab h-full min-h-0 flex bg-background">
      <div className="ftree-pane relative shrink-0 flex flex-col min-h-0 border-l border-l-border-variant border-r border-r-border-variant bg-background" ref={treeRef} style={{ width: treeWidth }}>
        <div className="ftree-resizer absolute -right-[3px] top-0 bottom-0 w-1.5 cursor-col-resize z-30 [&:hover]:bg-[color-mix(in_oklab,_var(--text)_12%,_transparent)] [&:active]:bg-[color-mix(in_oklab,_var(--text)_12%,_transparent)]" onPointerDown={resizeTree} />
        <div className="ftree-scroll flex-1 min-h-0 overflow-y-auto file-tree py-1.5 px-0 text-md">
          {tree(artifacts.entries)}
          {artifacts.truncated && (
            <p className="files-truncated m-0 py-2 px-3.5 text-xs text-muted">Listing truncated — the folder has more artifacts.</p>
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
        <div className="fpreview flex-1 min-w-0 flex flex-col min-h-0 bg-background fpreview-none items-center justify-center gap-2 text-md text-muted">
          <MousePointerClick size={22} strokeWidth={1.5} />
          <span>Click an artifact to view it</span>
        </div>
      )}
    </div>
  );
}
