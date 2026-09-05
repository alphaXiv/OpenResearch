// The compile side of a .tex tab: probe the machine for an engine, compile the
// file to a real PDF, and track whether that PDF still matches the source.
// Split out of FileViewer, which already carries three file sources, an editor
// and five render modes.

import { useCallback, useEffect, useRef, useState } from "react";
import { compileLatex, getLatexEngine } from "./api";
import { m } from "./paraglide/messages.js";

interface CompiledPdf {
  /** Checkout-relative path of the PDF, for the raw-file URL. */
  path: string;
  /** Cache-buster, so a recompile reloads the PDF without refetching the source. */
  version: number;
  /** Source it was built from — the only way to know it has gone stale. */
  source: string;
}

export interface LatexCompile {
  /** The engine that will run; null when the machine has none, undefined while probing. */
  engine: string | null | undefined;
  /** Install guidance, present only when there is no engine. */
  installHint: string | null;
  /** A paste-ready install command, where this platform has one. */
  installCommand: string | null;
  compiling: boolean;
  compiled: CompiledPdf | null;
  /** The PDF no longer matches the source in the editor. */
  stale: boolean;
  /** Engine output, present when the run reported errors. */
  log: string | null;
  /** A PDF was produced, but not cleanly. */
  builtWithErrors: boolean;
  /** The toolchain could not honour the document's requested engine. */
  note: string | null;
  error: string | null;
  showPdf: boolean;
  setShowPdf: (showPdf: boolean) => void;
  /** Bumped every time the PDF pane is shown. Toggling away unmounts the
   * viewer, and re-creating it on a URL the browser has already seen can leave
   * it blank — so each show gets a URL that is new. */
  viewNonce: number;
  compile: () => void;
  dismiss: () => void;
}

export function useLatexCompile({
  projectId,
  filePath,
  sessionId,
  enabled,
  autoRun = true,
  onManualAction,
  ready,
  source,
}: {
  projectId: string;
  /** The path that answered — an artifacts tab can resolve under another name. */
  filePath: string;
  sessionId?: string;
  /** This is a .tex file the compiler can actually reach (live checkout). */
  enabled: boolean;
  autoRun?: boolean;
  onManualAction?: () => void;
  /** The file has loaded, so `source` is real and not the empty initial buffer. */
  ready: boolean;
  /** The live edit buffer, which is what a compile should reflect. */
  source: string;
}): LatexCompile {
  const [engine, setEngine] = useState<string | null | undefined>(undefined);
  const [installHint, setInstallHint] = useState<string | null>(null);
  const [installCommand, setInstallCommand] = useState<string | null>(null);
  const [compiling, setCompiling] = useState(false);
  const [compiled, setCompiled] = useState<CompiledPdf | null>(null);
  const [log, setLog] = useState<string | null>(null);
  const [builtWithErrors, setBuiltWithErrors] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showPdf, setShowPdf] = useState(false);
  const [viewNonce, setViewNonce] = useState(0);
  const showPdfPane = useCallback((next: boolean) => {
    setShowPdf(next);
    if (next) setViewNonce((n) => n + 1);
  }, []);

  // Read at call time, so typing doesn't rebuild `compile` on every keystroke.
  const sourceRef = useRef(source);
  sourceRef.current = source;

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    getLatexEngine()
      .then((result) => {
        if (cancelled) return;
        setEngine(result.engine);
        setInstallHint(result.hint);
        setInstallCommand(result.installCommand);
      })
      .catch(() => {
        if (!cancelled) setEngine(null);
      });
    return () => {
      cancelled = true;
    };
  }, [enabled]);

  // The in-flight guard lives in a ref, not in a setState updater: React
  // double-invokes updaters under StrictMode, so a guard inside one lets two
  // compiles of the same file race each other's aux and output files.
  const compilingRef = useRef(false);
  const autoCompiled = useRef<string | null>(null);
  const compile = useCallback(() => {
    if (compilingRef.current) return;
    autoCompiled.current = filePath;
    compilingRef.current = true;
    setCompiling(true);
    const built = sourceRef.current;
    setError(null);
    setLog(null);
    setNote(null);
    compileLatex(projectId, filePath, { sessionId })
      .then((result) => {
        const pdfPath = result.pdfPath;
        if (result.ok && pdfPath) {
          setCompiled((prev) => ({
            path: pdfPath,
            version: (prev?.version ?? 0) + 1,
            source: built,
          }));
          setBuiltWithErrors(result.hadErrors);
          setNote(result.note);
          // A document that built despite errors still shows the log — the
          // errors are usually why a reference or a float looks wrong.
          if (result.hadErrors) setLog(result.log?.trim() || null);
          showPdfPane(true);
          return;
        }
        setCompiled(null);
        setBuiltWithErrors(false);
        setNote(result.note);
        setShowPdf(false);
        setLog(result.log?.trim() || m.latex_no_pdf_or_log());
      })
      .catch((e: unknown) => {
        setCompiled(null);
        setBuiltWithErrors(false);
        setNote(null);
        setShowPdf(false);
        setError(e instanceof Error ? e.message : String(e));
      })
      .finally(() => {
        compilingRef.current = false;
        setCompiling(false);
      });
  }, [projectId, filePath, sessionId, showPdfPane]);

  // Render the real document on open. Once per file: a compile that fails must
  // not spin, and the user can retry from the header.
  useEffect(() => {
    if (!enabled || !autoRun || !ready || !engine) return;
    if (autoCompiled.current === filePath) return;
    compile();
  }, [enabled, autoRun, ready, engine, filePath, compile]);

  return {
    engine,
    installHint,
    installCommand,
    compiling,
    compiled,
    stale: compiled !== null && compiled.source !== source,
    log,
    builtWithErrors,
    note,
    error,
    showPdf,
    setShowPdf: showPdfPane,
    viewNonce,
    compile: () => {
      onManualAction?.();
      compile();
    },
    dismiss: () => {
      setError(null);
      setLog(null);
    },
  };
}
