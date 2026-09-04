import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
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
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  absoluteFileUrl,
  artifactUrl,
  FileChangedError,
  getAbsoluteFile,
  getArtifactFileMetadata,
  getArtifactFileText,
  getProjectFile,
  openFileInEditor,
  projectFileUrl,
  saveProjectFile,
  type AbsoluteFile,
  type ArtifactEntry,
  type CheckoutRoot,
  type ProjectFile,
} from "../api";
import { useSessionBusyRefresh } from "../events";
import {
  conflictAfterRefresh,
  createFileBuffer,
  fileBufferContent,
  isDirtyFileBuffer,
  normalizedFileContent,
  type FileBufferState,
} from "../fileSync";
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
import { isHtmlFile, isLatexFile, isMarkdownFile } from "./FileTypeIcon";
import { HtmlPreview } from "./HtmlPreview";
import { OverleafPanel } from "./OverleafPanel";
import { MediaPreview, mediaPreviewKind } from "./MediaPreview";
import { Md } from "./Md";
import { Button, IconButton, IconButtonLink, Spinner } from "./ui";

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
      <IconButton
        data-tip={
          state === "copied"
            ? m.common_copied()
            : state === "select"
              ? m.file_viewer_selected_copy_shortcut()
              : m.file_viewer_copy_command()
        }
        aria-label={m.file_viewer_copy_install_command()}
        onClick={() => void copy()}
      >
        {state === "copied" ? <Check size={13} /> : <Copy size={13} />}
      </IconButton>
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
  artifactVersion,
  artifactEntries = [],
  initialBuffer,
  onBufferStateChange,
  remote = false,
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
  /** Selected artifact metadata changes only when this path changes. */
  artifactVersion?: string | null;
  artifactEntries?: ArtifactEntry[];
  initialBuffer?: FileBufferState;
  onBufferStateChange?: (buffer: FileBufferState | null) => void;
  remote?: boolean;
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
  // .html likewise, and its scripts run — see HtmlPreview.
  const isHtml = isHtmlFile(path);
  const rendersByDefault = isMarkdown || isHtml;
  const [showSource, setShowSource] = useState(false);
  // Live edit buffer for the code file. It IS the view for editable files (no
  // edit mode); it tracks the loaded content and diverges as the user types.
  const [editState, setEditState] = useState<FileBufferState | null>(initialBuffer ?? null);
  const editStateRef = useRef(editState);
  const onBufferStateChangeRef = useRef(onBufferStateChange);
  onBufferStateChangeRef.current = onBufferStateChange;
  const updateEditState = (next: FileBufferState | null) => {
    editStateRef.current = next;
    setEditState(next);
    onBufferStateChangeRef.current?.(
      next && (isDirtyFileBuffer(next) || next.conflict) ? next : null,
    );
  };
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [reloadPrompt, setReloadPrompt] = useState(false);
  const loadRequestRef = useRef(0);
  const bodyRef = useRef<HTMLDivElement>(null);
  const scrollPositionRef = useRef(scrollPosition);
  const data = loaded?.file ?? null;
  // A cited `artifacts/…` file can answer from either name in the checkout, so
  // writes, the editor, and raw bytes must target the path that answered.
  const filePath = editState && isDirtyFileBuffer(editState)
    ? editState.path
    : loaded?.source === "checkout" ? loaded.file.path : path;
  // This file's parent dir: the artifact report folder for image resolution,
  // and the anchor for a relative link inside an abs file.
  const parentFolder = filePath.split("/").slice(0, -1).join("/");
  // An artifacts tab that fell back must render as the checkout, not the store.
  const artifactsMode = loaded?.source === "artifact";
  const resolveMarkdownFilePath = useCallback(
    (target: string) =>
      resolveMarkdownTarget(parentFolder, target, isAbsolute)?.path ?? null,
    [isAbsolute, parentFolder],
  );
  /** Raw bytes of a file in whichever store answered for this tab. */
  const rawFileUrl = useCallback(
    (target: string) =>
      isAbsolute
        ? absoluteFileUrl(target)
        : artifactsMode
          ? artifactUrl(projectId, target)
          : projectFileUrl(projectId, target, { sessionId, ref: gitRef }),
    [artifactsMode, gitRef, isAbsolute, projectId, sessionId],
  );
  const resolveAssetSrc = useCallback(
    (src: string) => {
      if (isExternalMarkdownTarget(src)) return src;
      const target = resolveMarkdownTarget(parentFolder, src, isAbsolute);
      if (!target) return null;
      return markdownTargetUrl(rawFileUrl(target.path), target);
    },
    [isAbsolute, parentFolder, rawFileUrl],
  );
  const mediaKind = mediaPreviewKind(data?.presentation);
  const viaArtifacts = loaded?.source === "artifact" && !isArtifacts;
  const viaCheckout = isArtifacts && loaded?.source === "checkout";
  // A file that exists in the live checkout on disk (not a committed branch tree
  // or an artifact) — the only source the write/open endpoints can resolve.
  const onDisk = !gitRef && loaded?.source === "checkout" && data != null && !data.notFound;
  // A session read that fell back to the clone isn't the worktree it names: the
  // write and compile endpoints both refuse it, so nothing may act on it.
  const viaPrunedWorktree =
    sessionId != null && loaded?.source === "checkout" && loaded.file.root === "clone";
  const editableText =
    onDisk &&
    data != null &&
    !data.binary &&
    !data.truncated &&
    !mediaKind &&
    !viaPrunedWorktree;
  const unsafeRemoteEdit = editableText && data.version === undefined;
  const hasDraft = editState !== null && isDirtyFileBuffer(editState);
  const editable = !unsafeRemoteEdit && (
    (editableText && typeof data.version === "string") || hasDraft
  );
  // The editor replaces the read-only view for editable files — except markdown,
  // which stays rendered until its source toggle is on.
  // A <textarea> normalizes line endings to LF, so track the buffer in LF and
  // re-apply the file's original EOL on write (else a CRLF file's every line flips).
  const draft = editState?.draft ?? normalizedFileContent(data?.content ?? "");
  const baseline = editState?.baseline ?? normalizedFileContent(data?.content ?? "");
  const dirty = editable && editState !== null && isDirtyFileBuffer(editState);

  // Clean loads seed the editor. Dirty buffers survive tab switches and any
  // incoming disk version is handled by the guarded loader below.
  useEffect(() => {
    if (!editable || loaded?.source !== "checkout" || typeof data?.version !== "string") return;
    const current = editStateRef.current;
    if (current && isDirtyFileBuffer(current)) return;
    updateEditState(createFileBuffer(data.path, data.content, data.version));
    setSaveError(null);
  }, [data?.content, data?.version, editable, loaded?.source, path]);

  const save = async (expectedVersion?: string): Promise<boolean> => {
    const savingState = editStateRef.current;
    if (!editable || !savingState || !isDirtyFileBuffer(savingState)) return true;
    if (savingRef.current) return false;
    if (savingState.conflict && expectedVersion === undefined) {
      setSaveError(savingState.conflict.exists
        ? m.file_viewer_changed_on_disk()
        : m.file_viewer_deleted_on_disk());
      return false;
    }
    const savedDraft = savingState.draft;
    const content = fileBufferContent(savingState);
    savingRef.current = true;
    setSaving(true);
    setSaveError(null);
    try {
      const result = await saveProjectFile(projectId, filePath, content, {
        sessionId,
        expectedVersion: expectedVersion ?? savingState.version,
      });
      const current = editStateRef.current ?? savingState;
      loadRequestRef.current++;
      setLoading(false);
      updateEditState({
        ...current,
        baseline: savedDraft,
        version: result.version,
        conflict: null,
      });
      setLoaded((prev) =>
        prev && prev.source === "checkout"
          ? { source: "checkout", file: { ...prev.file, content, version: result.version } }
          : prev,
      );
      return true;
    } catch (e) {
      if (e instanceof FileChangedError) {
        const current = editStateRef.current ?? savingState;
        if (!isDirtyFileBuffer(current)) return false;
        updateEditState({
          ...current,
          conflict: { currentVersion: e.currentVersion, exists: e.exists },
        });
        return false;
      }
      setSaveError(e instanceof Error ? e.message : String(e));
      return false;
    } finally {
      savingRef.current = false;
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
    ? m.overleaf_sync_failed()
    : overleafConflicts > 0
      ? m.overleaf_conflict_tip()
      : overleaf.blocked
        ? m.overleaf_save_to_sync()
        : overleaf.link
          ? m.overleaf_in_sync()
          : m.overleaf_send_paper();
  const overleafColor =
    overleaf.error || overleafConflicts > 0
      ? "text-accent-red"
      : overleaf.link
        ? "text-accent-green"
        : undefined;

  // A .tex shows its compiled PDF or its source — nothing in between.
  const showingPdf = isLatex && latex.showPdf && latex.compiled != null;
  const showingUnsafeDraft = unsafeRemoteEdit && hasDraft;
  const showingEditor = (editable || showingUnsafeDraft) &&
    !(rendersByDefault && !showSource) &&
    !showingPdf;

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
  const reload = useCallback(() => {
    if (!savingRef.current) setNonce((value) => value + 1);
  }, []);
  const discardAndReload = () => {
    updateEditState(null);
    setReloadPrompt(false);
    setSaveError(null);
    reload();
  };

  useSessionBusyRefresh(
    projectId,
    sessionId,
    !gitRef && loaded?.source === "checkout",
    reload,
  );

  useEffect(() => {
    if (gitRef || !loaded || loaded.source === "artifact") return;
    window.addEventListener("focus", reload);
    return () => window.removeEventListener("focus", reload);
  }, [gitRef, loaded?.source, reload]);

  // filePath is `path` in every store but the checkout, so this covers all four.
  const rawUrl = `${rawFileUrl(filePath)}&v=${artifactsMode ? artifactVersion ?? nonce : nonce}`;

  useEffect(() => {
    let cancelled = false;
    const request = ++loadRequestRef.current;
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
        if (cancelled || request !== loadRequestRef.current) return;
        const current = editStateRef.current;
        if (current && isDirtyFileBuffer(current)) {
          const checkout = next.source === "checkout" ? next.file : null;
          const sameTarget = checkout !== null &&
            checkout.path === current.path &&
            (!sessionId || checkout.root === "worktree");
          const conflict = conflictAfterRefresh(
            current,
            sameTarget && typeof checkout.version === "string" ? checkout.version : null,
            sameTarget && !checkout.notFound,
          );
          if (
            conflict &&
            (conflict.currentVersion !== current.conflict?.currentVersion ||
              conflict.exists !== current.conflict?.exists)
          ) {
            updateEditState({ ...current, conflict });
          }
          else if (current.conflict) updateEditState({ ...current, conflict: null });
          setLoaded(next);
          setError(null);
          return;
        }
        setLoaded(next);
        setError(null);
      })
      .catch((e: Error) => {
        if (!cancelled && request === loadRequestRef.current) setError(e.message);
      })
      .finally(() => {
        if (!cancelled && request === loadRequestRef.current) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, path, source, sessionId, gitRef, nonce, artifactVersion]);

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
    if (d.source === "absolute") return m.file_viewer_not_found_on_disk();
    if (isArtifacts)
      return m.file_viewer_not_found_artifacts_or_root({ root: sessionId ? m.file_viewer_session_worktree() : m.file_viewer_project_clone() });
    if (gitRef) return m.file_viewer_not_found_on_branch({ branch: ltr(gitRef) });
    if (sessionId && d.source === "checkout" && d.file.root === "clone")
      return m.file_viewer_worktree_unavailable_not_found();
    const root = d.source === "checkout" ? d.file.root : d.checkoutRoot;
    return m.file_viewer_not_found_root_or_artifacts({ root: root === "worktree" ? m.file_viewer_session_worktree() : m.file_viewer_project_clone() });
  };

  return (
    <div className="file-view flex flex-col h-full min-h-0">
      <div className="file-view-header flex items-center gap-2 py-1.5 px-3 border-b border-b-border-variant text-text shrink-0">
        <FileText size={13} className="shrink-0" />
        <code className="file-view-path font-mono text-sm text-text flex-1 min-w-0 overflow-hidden text-ellipsis whitespace-nowrap" title={filePath}>
          {filePath}
        </code>
        {branchLabel && (
          <span className="file-view-branch inline-flex items-center gap-1 min-w-0 text-xs text-muted border border-border-variant rounded-sm py-px px-1.5 max-w-65 overflow-hidden text-ellipsis whitespace-nowrap shrink-0 [&_svg]:flex-none" title={m.a11y_branch({ branch: ltr(branchLabel) })}>
            <GitBranch size={11} />
            {branchLabel}
          </span>
        )}
        {showingEditor && (saving || dirty || saveError) && (
          <span
            className={`file-view-save-status inline-flex items-center gap-1 text-sm shrink-0 ${saveError ? "text-accent-red" : "text-muted"}`}
            title={saveError ?? (saving ? m.common_saving() : m.file_viewer_unsaved_tip())}
          >
            {saving ? (
              <>
                <Spinner /> {m.file_viewer_saving()}
              </>
            ) : saveError ? (
              m.file_viewer_save_failed()
            ) : (
              m.file_viewer_unsaved()
            )}
          </span>
        )}
        {isLatex && latex.compiled && (
          <IconButton
            active={!latex.showPdf}
            data-tip={
              latex.stale && latex.showPdf
                ? m.file_viewer_pdf_out_of_date()
                : latex.showPdf
                  ? m.common_view_source()
                  : m.file_viewer_show_compiled_pdf()
            }
            data-tip-align="end"
            aria-label={latex.showPdf ? m.common_view_source() : m.file_viewer_show_compiled_pdf()}
            onClick={() => latex.setShowPdf(!latex.showPdf)}
          >
            {latex.showPdf ? (
              <Code size={13} />
            ) : (
              <FileText size={13} className={latex.stale ? "text-accent-amber" : undefined} />
            )}
          </IconButton>
        )}
        {isLatex && compiledPdfUrl && compiledPdfName && (
          <IconButtonLink
            data-tip={
              latex.stale
                ? m.file_viewer_download_stale_pdf({ name: ltr(compiledPdfName) })
                : m.a11y_download_file({ name: ltr(compiledPdfName) })
            }
            data-tip-align="end"
            aria-label={m.a11y_download_file({ name: ltr(compiledPdfName) })}
            href={compiledPdfUrl}
            download={compiledPdfName}
          >
            <Download size={13} className={latex.stale ? "text-accent-amber" : undefined} />
          </IconButtonLink>
        )}
        {liveTex && (
          <IconButton
            active={showOverleaf}
            data-tip={overleafTip}
            data-tip-align="end"
            aria-label={m.a11y_overleaf_status({ status: overleafTip })}
            aria-expanded={showOverleaf}
            onClick={() => setShowOverleaf((open) => !open)}
          >
            {overleaf.syncing ? (
              <Spinner />
            ) : (
              <CloudUpload size={13} className={overleafColor} />
            )}
          </IconButton>
        )}
        {isLatex && onDisk && (
          <IconButton
            data-tip={latex.compiled ? m.file_viewer_recompile_pdf() : m.file_viewer_compile_pdf()}
            data-tip-align="end"
            aria-label={latex.compiled ? m.file_viewer_recompile_pdf() : m.file_viewer_compile_pdf()}
            disabled={latex.compiling || !latex.engine}
            onClick={() => void compileFromDisk()}
          >
            {latex.compiling ? <Spinner /> : <FileOutput size={13} />}
          </IconButton>
        )}
        {rendersByDefault && (
          <IconButton
            active={showSource}
            data-tip={showSource ? m.common_rendered_view() : m.common_view_source()}
            data-tip-align="end"
            aria-label={showSource ? m.common_rendered_view() : m.common_view_source()}
            onClick={() => setShowSource((s) => !s)}
          >
            <Code size={13} />
          </IconButton>
        )}
        {onDisk && !remote && (
          <IconButton
            data-tip={editorError ?? m.file_viewer_open_in_default_editor()}
            data-tip-align="end"
            aria-label={m.file_viewer_open_in_default_editor()}
            disabled={openingEditor}
            onClick={() => void openInEditor()}
          >
            {openingEditor ? <Spinner /> : <ExternalLink size={13} />}
          </IconButton>
        )}
        {!gitRef && loaded?.source !== "artifact" && (
          <IconButton
            data-tip={m.file_viewer_reload_file()}
            data-tip-align="end"
            aria-label={m.file_viewer_reload_file()}
            onPointerDown={(event) => {
              if (dirty) event.preventDefault();
            }}
            onClick={() => dirty ? setReloadPrompt(true) : reload()}
          >
            {loading ? <Spinner /> : <RotateCw size={13} />}
          </IconButton>
        )}
      </div>
      {/* Outside the scroll body, unlike its siblings: this state can be
          editable, and the editor's `h-full` would push it out of view. */}
      {!error && viaCheckout && loaded?.source === "checkout" && (
        <div className="file-view-note py-2.5 px-4 text-sm text-muted border-b border-b-border-variant shrink-0">
          {m.file_viewer_not_in_artifacts_showing_root({ root: loaded.file.root === "worktree" ? m.file_viewer_session_worktree() : m.file_viewer_project_clone() })}
        </div>
      )}
      {unsafeRemoteEdit && (
        <div className="file-view-note shrink-0 border-b border-b-border-variant py-2.5 px-4 text-sm text-accent-amber">
          {m.file_viewer_update_remote_to_edit_safely()}
        </div>
      )}
      {(editState?.conflict || reloadPrompt) && (
        <div
          className="file-view-note shrink-0 border-b border-b-border-variant py-2.5 px-4 flex items-center flex-wrap gap-2 text-sm text-accent-amber"
        >
          <span className="flex-1 min-w-0" role="status">
            {reloadPrompt && !editState?.conflict
              ? m.file_viewer_reload_discards_edits()
              : editState?.conflict?.exists
                ? m.file_viewer_changed_on_disk()
                : m.file_viewer_deleted_on_disk()}
          </span>
          {editState?.conflict?.exists && editState.conflict.currentVersion && (
            <Button
              disabled={saving}
              onPointerDown={(event) => event.preventDefault()}
              onClick={() => void save(editState.conflict?.currentVersion ?? undefined)}
            >
              {m.file_viewer_overwrite_disk_file()}
            </Button>
          )}
          {reloadPrompt && !editState?.conflict && (
            <Button onPointerDown={(event) => event.preventDefault()} onClick={() => setReloadPrompt(false)}>
              {m.chat_panel_cancel()}
            </Button>
          )}
          <Button disabled={saving} onPointerDown={(event) => event.preventDefault()} onClick={discardAndReload}>
            {m.file_viewer_reload_from_disk()}
          </Button>
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
                  ? m.file_viewer_compiled_with_errors()
                  : m.file_viewer_compile_failed())}
            </span>
            <IconButton
              data-tip={m.file_viewer_dismiss()}
              data-tip-align="end"
              aria-label={m.file_viewer_dismiss_compile_message()}
              onClick={latex.dismiss}
            >
              <X size={13} />
            </IconButton>
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
            {m.file_viewer_overleaf_apos_s_copy_of_this_file_was()}
          </span>
          <Button
            onClick={() => {
              overleaf.reloaded();
              setNonce((n) => n + 1);
            }}
          >
            {m.file_viewer_discard_my_edits_and_reload()}
          </Button>
        </div>
      )}
      {liveTex && overleaf.error && (
        <div className="file-view-note shrink-0 max-h-45 overflow-auto border-b border-b-border-variant py-2.5 px-4 flex items-start gap-2">
          <span className="flex-1 min-w-0 text-sm text-accent-red whitespace-pre-wrap">
            {overleaf.error}
          </span>
          <IconButton
            data-tip={m.file_viewer_dismiss()}
            data-tip-align="end"
            aria-label={m.file_viewer_dismiss_overleaf_message()}
            onClick={overleaf.dismiss}
          >
            <X size={13} />
          </IconButton>
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
          {m.file_viewer_this_pdf_was_compiled_from_an_earlier_version()}
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
            {m.file_viewer_this_session_apos_s_worktree_isn_apos_t()}
          </div>
        )}
        {!showingEditor && !error && loaded?.source === "artifact" && !loaded.file.notFound && viaArtifacts && (
          <div className="file-view-note py-2.5 px-4 text-sm text-muted">
            {m.file_viewer_artifact_fallback({ root: loaded.checkoutRoot === "worktree" ? m.file_viewer_session_worktree() : m.file_viewer_project_clone() })}
          </div>
        )}
        {error ? (
          <div className="file-view-note py-2.5 px-4 text-sm text-muted">{m.file_viewer_failed_to_load_file()} {ltr(error)}</div>
        ) : data === null ? (
          <div className="file-view-note py-2.5 px-4 text-sm text-muted">{m.file_viewer_loading()}</div>
        ) : showingEditor ? (
          // Editable files open straight into the editor — click and type.
          <CodeEditor
            value={draft}
            onChange={(next) => {
              const current = editStateRef.current ?? (
                data && typeof data.version === "string"
                  ? createFileBuffer(data.path, data.content, data.version)
                  : null
              );
              if (current) updateEditState({ ...current, draft: next });
              onEdit?.();
              if (saveError) setSaveError(null);
            }}
            onSave={() => void saveAndCompile()}
            onBlur={() => void saveAndCompile()}
            readOnly={showingUnsafeDraft}
            path={path}
            highlightLine={line}
            scrollRequest={lineScrollRequest}
            onScrollRequestHandled={onLineScrollRequestHandled}
         />
        ) : data.notFound ? (
          <div className="file-view-note py-2.5 px-4 text-sm text-muted">
            {loaded ? notFoundCopy(loaded) : m.file_viewer_not_found()}
          </div>
        ) : mediaKind ? (
          <MediaPreview
            kind={mediaKind}
            url={rawUrl}
            name={path.split("/").pop() ?? path}
         />
        ) : data.binary ? (
          <div className="file-view-note py-2.5 px-4 text-sm text-muted">
            {m.file_viewer_binary_file_no_inline_preview()} <a href={rawUrl} download={path.split("/").pop() ?? path}>{m.file_viewer_download()}</a>
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
          <div className="file-view-md max-w-readable pt-4.5 px-5 pb-8 [&_.md]:text-base [&_.md_h1]:text-2xl [&_.md_h1]:mt-4.5 [&_.md_h1]:mx-0 [&_.md_h1]:mb-2 [&_.md_h2]:text-xl [&_.md_h2]:mt-4 [&_.md_h2]:mx-0 [&_.md_h2]:mb-2 [&_.md_h3]:text-lg">
            {artifactsMode ? (
              <ArtifactMarkdown
                projectId={projectId}
                folder={parentFolder}
                markdown={data.content}
                entries={artifactEntries}
             />
            ) : (
              <Md
                text={data.content}
                resolveFilePath={resolveMarkdownFilePath}
                resolveImageSrc={resolveAssetSrc}
                onOpenFile={
                  onOpenFile &&
                  ((p, _line, _exp, _ref, intent) =>
                    onOpenFile(p, sessionId, gitRef, intent))
                }
             />
            )}
          </div>
        ) : isHtml && !showSource ? (
          <HtmlPreview
            html={data.content}
            truncated={data.truncated}
            url={rawUrl}
            name={filePath}
            resolveSrc={resolveAssetSrc}
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
              <div className="file-view-note py-2.5 px-4 text-sm text-muted">{m.file_viewer_file_truncated_showing_the_first_512_kb()}</div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
