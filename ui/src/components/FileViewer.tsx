// Mirror of openresearch.sh's AgentFileView: one file from the project —
// a branch's committed copy when the tab carries a ref, else the chat
// session's worktree, else the hub clone, else the project's artifacts, with
// an artifacts tab falling back the other way when only the checkout has it —
// refractor-highlighted, opened as a right-pane tab from chat tool rows or
// the code browser.

import {
  Check,
  CloudUpload,
  Code,
  Copy,
  Download,
  ExternalLink,
  FileOutput,
  FileText,
  GitBranch,
  RotateCw,
  X,
} from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  absoluteFileUrl,
  artifactUrl,
  getAbsoluteFile,
  getArtifactFileMetadata,
  getArtifactFileText,
  getProjectFile,
  openFileInEditor,
  projectFileUrl,
  saveProjectFile,
  type AbsoluteFile,
  type CheckoutRoot,
  type ProjectFile,
} from "../api";
import { useLatexCompile } from "../useLatexCompile";
import { useOverleafSync } from "../useOverleafSync";
import {
  isExternalMarkdownTarget,
  markdownTargetUrl,
  resolveMarkdownTarget,
} from "../markdownTarget";
import { CodeView } from "./CodeView";
import { CodeEditor } from "./CodeEditor";
import { ArtifactMarkdown } from "./ArtifactsTab";
import type { TabOpenIntent } from "../tabPreview";
import { isLatexFile, isMarkdownFile } from "./FileTypeIcon";
import { OverleafPanel } from "./OverleafPanel";
import { MediaPreview, mediaPreviewKind } from "./MediaPreview";
import { Md } from "./Md";
import { BUTTON_CLASS_NAME, ICON_BUTTON_CLASS_NAME, SPINNER_CLASS_NAME } from "../styleClasses";

type ArtifactPreviewFile = Omit<ProjectFile, "root">;
type LoadedFile =
  | { source: "checkout"; file: ProjectFile }
  | { source: "artifact"; file: ArtifactPreviewFile; checkoutRoot?: CheckoutRoot }
  | { source: "absolute"; file: AbsoluteFile };

export interface FileScrollPosition {
  top: number;
  left: number;
}

/** An install command with a one-click copy — the point of showing it is that
 * the user runs it in a terminal. */
function CopyableCommand({ command }: { command: string }) {
  const [state, setState] = useState<"idle" | "copied" | "select">("idle");
  const codeRef = useRef<HTMLElement>(null);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(command);
      setState("copied");
      setTimeout(() => setState("idle"), 1500);
    } catch {
      // Blocked (permissions, an unfocused document, an old browser). Select
      // the text so the user can still take it — silently doing nothing on a
      // button whose whole job is copying is the one unacceptable outcome.
      const node = codeRef.current;
      if (node) {
        const range = document.createRange();
        range.selectNodeContents(node);
        const selection = window.getSelection();
        selection?.removeAllRanges();
        selection?.addRange(range);
      }
      setState("select");
      setTimeout(() => setState("idle"), 4000);
    }
  };

  return (
    <div className="mt-2 flex items-center gap-2">
      <code
        ref={codeRef}
        className="font-mono text-xs text-text bg-panel border border-border-variant rounded-xs py-1 px-2"
      >
        {command}
      </code>
      <button
        className={ICON_BUTTON_CLASS_NAME}
        data-tip={
          state === "copied"
            ? "Copied"
            : state === "select"
              ? "Selected — press ⌘C"
              : "Copy command"
        }
        aria-label="Copy install command"
        onClick={() => void copy()}
      >
        {state === "copied" ? <Check size={13} /> : <Copy size={13} />}
      </button>
    </div>
  );
}

export function FileViewer({
  projectId,
  path,
  source = "repo",
  sessionId,
  gitRef,
  line,
  branchLabel,
  onOpenFile,
  scrollPosition,
  onScrollPositionChange,
  lineScrollRequest,
  onLineScrollRequestHandled,
  onEdit,
}: {
  projectId: string;
  path: string;
  /** Which backend serves this file. "artifacts" reads the project's durable
   * output directory; "abs" reads an absolute path off disk (a file outside
   * the checkout and artifacts); else the repo/worktree checkout. */
  source?: "repo" | "artifacts" | "abs";
  /** Chat session whose worktree holds the file (absent → hub clone). An
   * "artifacts" tab carries it only for the checkout fallback; "abs" never. */
  sessionId?: string;
  /** Branch whose committed copy to show — overrides the live checkout.
   * (Named gitRef because `ref` is reserved on React components.) */
  gitRef?: string;
  /** 1-based line to scroll to and highlight once the source renders. */
  line?: number;
  /** The git branch this file's contents came from (experiment branch, or the
   * baseline) — shown in the header so a code tab always names its branch. */
  branchLabel?: string;
  /** Open a linked file as another tab (rendered-markdown links). */
  onOpenFile?: (
    path: string,
    sessionId: string | undefined,
    ref: string | undefined,
    intent: TabOpenIntent,
  ) => void;
  scrollPosition?: FileScrollPosition;
  onScrollPositionChange?: (position: FileScrollPosition) => void;
  lineScrollRequest?: number;
  onLineScrollRequestHandled?: () => void;
  /** Typing in the editor — the commitment that takes a preview tab out of
   * preview mode. */
  onEdit?: () => void;
}) {
  const [loaded, setLoaded] = useState<LoadedFile | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [nonce, setNonce] = useState(0);
  const isArtifacts = source === "artifacts";
  const isAbsolute = source === "abs";
  // Markdown renders by default; the header toggle shows the raw source.
  const isMarkdown = isMarkdownFile(path);
  // .tex behaves like markdown — rendered by default, source behind the toggle —
  // and adds the real compiler on top.
  const isLatex = isLatexFile(path);
  const [showSource, setShowSource] = useState(false);
  // Live edit buffer for the code file. It IS the view for editable files (no
  // edit mode); it tracks the loaded content and diverges as the user types.
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const scrollPositionRef = useRef(scrollPosition);
  const data = loaded?.file ?? null;
  // A cited `artifacts/…` file can answer from either name in the checkout, so
  // writes, the editor, and raw bytes must target the path that answered.
  const filePath = loaded?.source === "checkout" ? loaded.file.path : path;
  // This file's parent dir: the artifact report folder for image resolution,
  // and the anchor for a relative link inside an abs file.
  const parentFolder = filePath.split("/").slice(0, -1).join("/");
  const resolveMarkdownFilePath = useCallback(
    (target: string) =>
      resolveMarkdownTarget(parentFolder, target, isAbsolute)?.path ?? null,
    [isAbsolute, parentFolder],
  );
  const resolveMarkdownImageSrc = useCallback(
    (src: string) => {
      if (isExternalMarkdownTarget(src)) return src;
      const target = resolveMarkdownTarget(parentFolder, src, isAbsolute);
      if (!target) return null;
      const url = isAbsolute
        ? absoluteFileUrl(target.path)
        : projectFileUrl(projectId, target.path, { sessionId, ref: gitRef });
      return markdownTargetUrl(url, target);
    },
    [gitRef, isAbsolute, parentFolder, projectId, sessionId],
  );
  const mediaKind = mediaPreviewKind(data?.presentation);
  const viaArtifacts = loaded?.source === "artifact" && !isArtifacts;
  const viaCheckout = isArtifacts && loaded?.source === "checkout";
  // An artifacts tab that fell back must render as the checkout, not the store.
  const artifactsMode = loaded?.source === "artifact";
  // A file that exists in the live checkout on disk (not a committed branch tree
  // or an artifact) — the only source the write/open endpoints can resolve.
  const onDisk = !gitRef && loaded?.source === "checkout" && data != null && !data.notFound;
  // Editable = a live checkout text file. A session read that fell back to the
  // clone isn't the worktree it names, so it stays read-only rather than
  // silently editing another checkout.
  // A session read that fell back to the clone isn't the worktree it names: the
  // write and compile endpoints both refuse it, so nothing may act on it.
  const viaPrunedWorktree =
    sessionId != null && loaded?.source === "checkout" && loaded.file.root === "clone";
  const editable =
    onDisk &&
    data != null &&
    !data.binary &&
    !data.truncated &&
    !mediaKind &&
    !viaPrunedWorktree;
  // The editor replaces the read-only view for editable files — except markdown,
  // which stays rendered until its source toggle is on.
  // A <textarea> normalizes line endings to LF, so track the buffer in LF and
  // re-apply the file's original EOL on write (else a CRLF file's every line flips).
  const baseline = useMemo(() => (data?.content ?? "").replace(/\r\n/g, "\n"), [data?.content]);
  const dirty = editable && draft !== baseline;

  // Reseed the buffer only on a genuine load/reload — skip the optimistic
  // baseline bump `save()` makes, so a keystroke typed mid-save isn't clobbered.
  const lastWriteRef = useRef<string | null>(null);
  useEffect(() => {
    const incoming = data?.content ?? "";
    if (lastWriteRef.current !== null && incoming === lastWriteRef.current) {
      lastWriteRef.current = null;
      return;
    }
    setDraft(incoming.replace(/\r\n/g, "\n"));
    setSaveError(null);
  }, [data?.content, path]);

  const save = async (): Promise<boolean> => {
    if (!editable || data == null || !dirty || saving) return !dirty;
    const content = data.content.includes("\r\n") ? draft.replace(/\n/g, "\r\n") : draft;
    setSaving(true);
    setSaveError(null);
    try {
      await saveProjectFile(projectId, filePath, content, { sessionId });
      // Advance the baseline to what we wrote so `dirty` clears without a refetch;
      // mark it so the reseed effect ignores this self-inflicted change.
      lastWriteRef.current = content;
      setLoaded((prev) =>
        prev && prev.source === "checkout"
          ? { source: "checkout", file: { ...prev.file, content } }
          : prev,
      );
      return true;
    } catch (e) {
      setSaveError(e instanceof Error ? e.message : String(e));
      return false;
    } finally {
      setSaving(false);
    }
  };

  // A .tex the compiler and the sync can both reach: the live checkout, which
  // is the same condition that makes the file editable in the first place.
  const liveTex = isLatex && onDisk && !viaPrunedWorktree;
  const latex = useLatexCompile({
    projectId,
    filePath,
    sessionId,
    enabled: liveTex,
    ready: data != null && !data.notFound,
    source: editable ? draft : (data?.content ?? ""),
  });

  // Not gated on a local engine: a machine with no TeX install can still send
  // the paper, and Overleaf compiles it there.
  const overleaf = useOverleafSync({
    projectId,
    filePath,
    sessionId,
    enabled: liveTex,
    savedSource: baseline,
    dirty,
    // A pull rewrote the file underneath this view; refetch so the editor shows
    // what is now on disk rather than the copy it loaded.
    onPulled: useCallback(
      (paths: string[]) => {
        if (paths.includes(filePath)) setNonce((n) => n + 1);
      },
      [filePath],
    ),
  });
  const [showOverleaf, setShowOverleaf] = useState(false);
  const overleafConflicts = overleaf.last?.conflicts.length ?? 0;
  // A conflict is the one outcome the user has to act on, and an automatic sync
  // can produce it with the panel closed.
  useEffect(() => {
    if (overleafConflicts > 0) setShowOverleaf(true);
  }, [overleafConflicts]);
  const overleafTip = overleaf.error
    ? "Overleaf sync failed"
    : overleafConflicts > 0
      ? "Changed here and on Overleaf — choose which copy to keep"
      : overleaf.blocked
        ? "Save this file to sync it with Overleaf"
        : overleaf.link
          ? "In step with Overleaf"
          : "Send this paper to Overleaf";
  const overleafColor =
    overleaf.error || overleafConflicts > 0
      ? "text-accent-red"
      : overleaf.link
        ? "text-accent-green"
        : undefined;

  // A .tex shows its compiled PDF or its source — nothing in between.
  const showingPdf = isLatex && latex.showPdf && latex.compiled != null;
  const showingEditor = editable && !(isMarkdown && !showSource) && !showingPdf;

  // `#toolbar=0` asks the browser's PDF viewer to drop its own chrome, so the
  // pane shows the document and this view's header owns the controls.
  const compiledPdfUrl = latex.compiled
    ? `${projectFileUrl(projectId, latex.compiled.path, { sessionId })}&v=${latex.compiled.version}`
    : null;
  // The viewer is re-created on every show, so the URL must differ each time —
  // same URL, new element leaves Chrome's PDF viewer blank.
  const pdfPaneUrl = compiledPdfUrl
    ? `${compiledPdfUrl}&view=${latex.viewNonce}#toolbar=0&navpanes=0&statusbar=0`
    : null;
  const compiledPdfName = latex.compiled
    ? (latex.compiled.path.split("/").pop() ?? latex.compiled.path)
    : null;

  // The compiler reads the file on disk, so anything that triggers a compile
  // flushes the buffer first. A failed save must not compile: the PDF would be
  // of the previous content while `stale` compared against the new draft.
  const compileFromDisk = async () => {
    if (dirty && !(await save())) return;
    if (isLatex && latex.engine) latex.compile();
  };
  // Blur and ⌘S only rebuild when there was an edit to save.
  const saveAndCompile = async () => {
    if (!dirty) return;
    await compileFromDisk();
  };

  const [openingEditor, setOpeningEditor] = useState(false);
  const [editorError, setEditorError] = useState<string | null>(null);
  // Hand the file to the OS, which opens it in the user's default app for the
  // type (their editor for source files) — no picker.
  const openInEditor = async () => {
    setOpeningEditor(true);
    setEditorError(null);
    try {
      await openFileInEditor(projectId, filePath, { sessionId });
    } catch (e) {
      setEditorError(e instanceof Error ? e.message : String(e));
    } finally {
      setOpeningEditor(false);
    }
  };
  const rawUrlBase = isAbsolute
    ? absoluteFileUrl(path)
    : artifactsMode
      ? artifactUrl(projectId, path)
      : projectFileUrl(projectId, filePath, { sessionId, ref: gitRef });
  const rawUrl = `${rawUrlBase}&v=${nonce}`;

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    // Artifacts come from the compatibility /files endpoint (no session/branch);
    // repo files from the checkout-aware /file endpoint. All paths normalize
    // into the same ProjectFile-shaped `data` so the render body is shared.
    const fromArtifacts = async (): Promise<ArtifactPreviewFile> => {
      const metadata = await getArtifactFileMetadata(projectId, path);
      const wantsBody = metadata?.presentation === "text" || metadata?.presentation === "unknown";
      const body = metadata && wantsBody
        ? await getArtifactFileText(projectId, path)
        : null;
      const notFound = metadata === null || (wantsBody && body === null);
      return {
        // A missing artifact resolves to null → notFound, so it shows
        // the friendly copy rather than a raw error.
        path,
        content: body?.content ?? "",
        truncated: body?.truncated ?? false,
        binary: body?.binary ?? metadata?.presentation === "download",
        notFound,
        presentation: body
          ? (body.binary ? "download" : "text")
          : (metadata?.presentation ?? "download"),
      };
    };
    // A cited artifact path arrives stripped of its `artifacts/` prefix, which
    // the checkout copy usually keeps — try that first; a throwing probe
    // (unknown session, directory name) just means "not here".
    const fromCheckout = async (): Promise<ProjectFile | null> => {
      for (const candidate of [`artifacts/${path}`, path]) {
        const file = await getProjectFile(projectId, candidate, { sessionId }).catch(() => null);
        if (file && !file.notFound) return file;
      }
      return null;
    };
    // Branch tabs do not fall back because a ref names a committed tree.
    const load: Promise<LoadedFile> = isAbsolute
      ? getAbsoluteFile(path).then((file) => ({ source: "absolute", file }))
      : isArtifacts
      ? fromArtifacts().then(async (file) => {
          if (!file.notFound) return { source: "artifact", file };
          const checkout = await fromCheckout();
          return checkout ? { source: "checkout", file: checkout } : { source: "artifact", file };
        })
      : getProjectFile(projectId, path, { sessionId, ref: gitRef }).then((d) =>
          d.notFound && !gitRef
            ? fromArtifacts().then((f) =>
                f.notFound
                  ? { source: "checkout", file: d }
                  : { source: "artifact", file: f, checkoutRoot: d.root },
              )
            : { source: "checkout", file: d },
        );
    load
      .then((next) => {
        if (cancelled) return;
        setLoaded(next);
        setError(null);
      })
      .catch((e: Error) => {
        if (!cancelled) setError(e.message);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, path, source, sessionId, gitRef, nonce]);

  // Stays a layout effect: the code views scroll to a `file:line` target in
  // passive effects, which run after this and so win over the restore.
  useLayoutEffect(() => {
    const body = bodyRef.current;
    const position = scrollPositionRef.current;
    if (!body || !data || !position) return;
    body.scrollTop = position.top;
    body.scrollLeft = position.left;
  }, [data]);

  const notFoundCopy = (d: LoadedFile) => {
    if (d.source === "absolute") return "File not found on disk.";
    if (isArtifacts)
      return `File not found in the project's artifacts or the ${
        sessionId ? "session's worktree" : "project clone"
      }.`;
    if (gitRef) return `File not found on branch ${gitRef}.`;
    if (sessionId && d.source === "checkout" && d.file.root === "clone")
      return "This session's worktree isn't available, and the file isn't in the project clone or its artifacts.";
    const root = d.source === "checkout" ? d.file.root : d.checkoutRoot;
    return `File not found in the ${root === "worktree" ? "session's worktree" : "project clone"} or the project's artifacts.`;
  };

  return (
    <div className="file-view flex flex-col h-full min-h-0">
      <div className="file-view-header flex items-center gap-2 py-1.5 px-3 border-b border-b-border-variant text-text shrink-0">
        <FileText size={13} style={{ flexShrink: 0 }} />
        <code className="file-view-path font-mono text-sm text-text flex-1 min-w-0 overflow-hidden text-ellipsis whitespace-nowrap" title={filePath}>
          {filePath}
        </code>
        {branchLabel && (
          <span className="file-view-branch inline-flex items-center gap-1 min-w-0 font-mono text-xs text-muted border border-border-variant rounded-sm py-px px-1.5 max-w-65 overflow-hidden text-ellipsis whitespace-nowrap shrink-0 [&_svg]:flex-none" title={`Branch: ${branchLabel}`}>
            <GitBranch size={11} />
            {branchLabel}
          </span>
        )}
        {showingEditor && (saving || dirty || saveError) && (
          <span
            className={`file-view-save-status inline-flex items-center gap-1 text-xs shrink-0 ${saveError ? "text-accent-red" : "text-muted"}`}
            title={saveError ?? (saving ? "Saving…" : "Unsaved — ⌘S or click away to save")}
          >
            {saving ? (
              <>
                <span className={SPINNER_CLASS_NAME} /> Saving…
              </>
            ) : saveError ? (
              "Save failed"
            ) : (
              "Unsaved"
            )}
          </span>
        )}
        {isLatex && latex.compiled && (
          <button
            className={`${ICON_BUTTON_CLASS_NAME} ${!latex.showPdf ? "active" : ""}`}
            data-tip={
              latex.stale && latex.showPdf
                ? "Compiled PDF is out of date"
                : latex.showPdf
                  ? "View source"
                  : "Show compiled PDF"
            }
            data-tip-align="end"
            aria-label={latex.showPdf ? "View source" : "Show compiled PDF"}
            onClick={() => latex.setShowPdf(!latex.showPdf)}
          >
            {latex.showPdf ? (
              <Code size={13} />
            ) : (
              <FileText size={13} className={latex.stale ? "text-accent-amber" : undefined} />
            )}
          </button>
        )}
        {isLatex && compiledPdfUrl && compiledPdfName && (
          <a
            className={ICON_BUTTON_CLASS_NAME}
            data-tip={
              latex.stale
                ? `Download ${compiledPdfName} (out of date — recompile first)`
                : `Download ${compiledPdfName}`
            }
            data-tip-align="end"
            aria-label={`Download ${compiledPdfName}`}
            href={compiledPdfUrl}
            download={compiledPdfName}
          >
            <Download size={13} className={latex.stale ? "text-accent-amber" : undefined} />
          </a>
        )}
        {liveTex && (
          <button
            className={`${ICON_BUTTON_CLASS_NAME} ${showOverleaf ? "active" : ""}`}
            data-tip={overleafTip}
            data-tip-align="end"
            aria-label={`Overleaf — ${overleafTip.toLowerCase()}`}
            aria-expanded={showOverleaf}
            onClick={() => setShowOverleaf((open) => !open)}
          >
            {overleaf.syncing ? (
              <span className={SPINNER_CLASS_NAME} />
            ) : (
              <CloudUpload size={13} className={overleafColor} />
            )}
          </button>
        )}
        {isLatex && onDisk && (
          <button
            className={ICON_BUTTON_CLASS_NAME}
            data-tip={latex.compiled ? "Recompile PDF" : "Compile PDF"}
            data-tip-align="end"
            aria-label={latex.compiled ? "Recompile PDF" : "Compile PDF"}
            disabled={latex.compiling || !latex.engine}
            onClick={() => void compileFromDisk()}
          >
            {latex.compiling ? <span className={SPINNER_CLASS_NAME} /> : <FileOutput size={13} />}
          </button>
        )}
        {isMarkdown && (
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
        {onDisk && (
          <button
            className={ICON_BUTTON_CLASS_NAME}
            data-tip={editorError ?? "Open in default editor"}
            data-tip-align="end"
            aria-label="Open in default editor"
            disabled={openingEditor}
            onClick={() => void openInEditor()}
          >
            {openingEditor ? <span className={SPINNER_CLASS_NAME} /> : <ExternalLink size={13} />}
          </button>
        )}
        <button
          className={ICON_BUTTON_CLASS_NAME}
          data-tip="Reload file"
          data-tip-align="end"
          aria-label="Reload file"
          onClick={() => setNonce((n) => n + 1)}
        >
          {loading ? <span className={SPINNER_CLASS_NAME} /> : <RotateCw size={13} />}
        </button>
      </div>
      {/* Outside the scroll body, unlike its siblings: this state can be
          editable, and the editor's `h-full` would push it out of view. */}
      {!error && viaCheckout && loaded?.source === "checkout" && (
        <div className="file-view-note py-2.5 px-4 text-sm text-muted border-b border-b-border-variant shrink-0">
          Not in the project&apos;s artifacts — showing the copy from the{" "}
          {loaded.file.root === "worktree" ? "session's worktree" : "project clone"}.
        </div>
      )}
      {(latex.error || latex.log) && (
        <div className="file-view-note shrink-0 max-h-45 overflow-auto border-b border-b-border-variant py-2.5 px-4">
          <div className="flex items-start gap-2">
            <span
              className={`flex-1 min-w-0 text-sm ${
                latex.builtWithErrors ? "text-subtext" : "text-accent-red"
              }`}
            >
              {latex.error ??
                (latex.builtWithErrors
                  ? "Compiled, but the engine reported errors — check the output below."
                  : "Compile failed")}
            </span>
            <button
              className={ICON_BUTTON_CLASS_NAME}
              data-tip="Dismiss"
              data-tip-align="end"
              aria-label="Dismiss compile message"
              onClick={latex.dismiss}
            >
              <X size={13} />
            </button>
          </div>
          {latex.log && (
            <pre className="mt-1.5 mb-0 font-mono text-xs text-subtext whitespace-pre-wrap wrap-anywhere">
              {latex.log}
            </pre>
          )}
        </div>
      )}
      {liveTex && overleaf.staleOnDisk && (
        <div className="file-view-note shrink-0 border-b border-b-border-variant py-2.5 px-4 flex items-center flex-wrap gap-2 text-sm text-accent-amber">
          <span className="flex-1 min-w-0">
            Overleaf&apos;s copy of this file was pulled while you had unsaved edits, so what you
            see is no longer what is on disk. Saving now sends this draft to Overleaf instead.
          </span>
          <button
            className={BUTTON_CLASS_NAME}
            onClick={() => {
              overleaf.reloaded();
              setNonce((n) => n + 1);
            }}
          >
            Discard my edits and reload
          </button>
        </div>
      )}
      {liveTex && overleaf.error && (
        <div className="file-view-note shrink-0 max-h-45 overflow-auto border-b border-b-border-variant py-2.5 px-4 flex items-start gap-2">
          <span className="flex-1 min-w-0 text-sm text-accent-red whitespace-pre-wrap">
            {overleaf.error}
          </span>
          <button
            className={ICON_BUTTON_CLASS_NAME}
            data-tip="Dismiss"
            data-tip-align="end"
            aria-label="Dismiss Overleaf message"
            onClick={overleaf.dismiss}
          >
            <X size={13} />
          </button>
        </div>
      )}
      {liveTex && showOverleaf && overleaf.loaded && (
        <div className="file-view-note shrink-0 border-b border-b-border-variant py-2.5 px-4">
          <OverleafPanel overleaf={overleaf} />
        </div>
      )}
      {isLatex && onDisk && latex.engine === null && latex.installHint && (
        <div className="file-view-note shrink-0 border-b border-b-border-variant py-2.5 px-4 text-sm text-subtext">
          {latex.installHint}
          {latex.installCommand && <CopyableCommand command={latex.installCommand} />}
        </div>
      )}
      {latex.note && (
        <div className="file-view-note shrink-0 border-b border-b-border-variant py-2 px-4 text-sm text-accent-amber">
          {latex.note}
        </div>
      )}
      {showingPdf && latex.stale && (
        <div className="file-view-note shrink-0 border-b border-b-border-variant py-2 px-4 text-sm text-subtext">
          This PDF was compiled from an earlier version of the source — recompile to update it.
        </div>
      )}
      <div
        ref={bodyRef}
        className="file-view-body flex-1 min-h-0 overflow-auto bg-background"
        onScroll={(event) => {
          const position = {
            top: event.currentTarget.scrollTop,
            left: event.currentTarget.scrollLeft,
          };
          scrollPositionRef.current = position;
          onScrollPositionChange?.(position);
        }}
      >
        {!showingEditor && !error && !isArtifacts && loaded?.source === "checkout" && !loaded.file.notFound && !gitRef && sessionId && loaded.file.root === "clone" && (
          <div className="file-view-note py-2.5 px-4 text-sm text-muted">
            This session&apos;s worktree isn&apos;t available — showing the project clone&apos;s copy.
          </div>
        )}
        {!showingEditor && !error && loaded?.source === "artifact" && !loaded.file.notFound && viaArtifacts && (
          <div className="file-view-note py-2.5 px-4 text-sm text-muted">
            Not in the {loaded.checkoutRoot === "worktree" ? "session's worktree" : "project clone"} —
            showing the copy from the project&apos;s artifacts.
          </div>
        )}
        {error ? (
          <div className="file-view-note py-2.5 px-4 text-sm text-muted">Failed to load file: {error}</div>
        ) : data === null ? (
          <div className="file-view-note py-2.5 px-4 text-sm text-muted">Loading…</div>
        ) : data.notFound ? (
          <div className="file-view-note py-2.5 px-4 text-sm text-muted">
            {loaded ? notFoundCopy(loaded) : "File not found."}
          </div>
        ) : mediaKind ? (
          <MediaPreview
            kind={mediaKind}
            url={rawUrl}
            name={path.split("/").pop() ?? path}
          />
        ) : data.binary ? (
          <div className="file-view-note py-2.5 px-4 text-sm text-muted">
            Binary file — no inline preview. <a href={rawUrl} download={path.split("/").pop() ?? path}>Download</a>
          </div>
        ) : showingPdf && pdfPaneUrl && compiledPdfName ? (
          <MediaPreview
            key={pdfPaneUrl}
            kind="pdf"
            url={pdfPaneUrl}
            name={compiledPdfName}
            downloadBar={false}
          />
        ) : isMarkdown && !showSource ? (
          <div className="file-view-md max-w-readable pt-4.5 px-5 pb-8 [&_.md]:text-base [&_.md_h1]:text-[1.5em] [&_.md_h1]:mt-4.5 [&_.md_h1]:mx-0 [&_.md_h1]:mb-2 [&_.md_h2]:text-[1.25em] [&_.md_h2]:mt-4 [&_.md_h2]:mx-0 [&_.md_h2]:mb-2 [&_.md_h3]:text-[1.1em]">
            {artifactsMode ? (
              <ArtifactMarkdown
                projectId={projectId}
                folder={parentFolder}
                markdown={data.content}
              />
            ) : (
              <Md
                text={data.content}
                resolveFilePath={resolveMarkdownFilePath}
                resolveImageSrc={resolveMarkdownImageSrc}
                onOpenFile={
                  onOpenFile &&
                  ((p, _line, _exp, _ref, intent) =>
                    onOpenFile(p, sessionId, gitRef, intent))
                }
              />
            )}
          </div>
        ) : showingEditor ? (
          // Editable files open straight into the editor — click and type.
          <CodeEditor
            value={draft}
            onChange={(next) => {
              setDraft(next);
              onEdit?.();
              if (saveError) setSaveError(null);
            }}
            onSave={() => void saveAndCompile()}
            onBlur={() => void saveAndCompile()}
            path={path}
            highlightLine={line}
            scrollRequest={lineScrollRequest}
            onScrollRequestHandled={onLineScrollRequestHandled}
          />
        ) : (
          <>
            <CodeView
              text={data.content}
              path={path}
              highlightLine={line}
              scrollRequest={lineScrollRequest}
              onScrollRequestHandled={onLineScrollRequestHandled}
            />
            {data.truncated && (
              <div className="file-view-note py-2.5 px-4 text-sm text-muted">File truncated — showing the first 512 KB.</div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
