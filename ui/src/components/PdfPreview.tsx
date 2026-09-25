// The legacy build polyfills the newest JS the modern one requires, so 2024-era
// Safari, WebKitGTK, and Chromium work; older engines fall back to the download link.
import "pdfjs-dist/legacy/web/pdf_viewer.css";
import { getDocument, PDFWorker, version } from "pdfjs-dist/legacy/build/pdf.mjs";
import PdfWorker from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?worker";
import {
  EventBus,
  LinkTarget,
  PDFFindController,
  PDFLinkService,
  PDFViewer,
} from "pdfjs-dist/legacy/web/pdf_viewer.mjs";
import { ChevronDown, ChevronUp, Minus, MoveHorizontal, Plus } from "lucide-react";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { fmtNumber } from "../i18n";
import { m } from "../paraglide/messages.js";
import { IconButton, Input } from "./ui";

// WebKit's streams aren't async-iterable, and PDF.js iterates one to extract page
// text; without this, find silently matches nothing in WKWebView and WebKitGTK.
if (!(Symbol.asyncIterator in ReadableStream.prototype)) {
  Object.defineProperty(ReadableStream.prototype, Symbol.asyncIterator, {
    configurable: true,
    writable: true,
    value: async function* (this: ReadableStream) {
      const reader = this.getReader();
      try {
        for (;;) {
          const { done, value } = await reader.read();
          if (done) return;
          yield value;
        }
      } finally {
        reader.releaseLock();
      }
    },
  });
}

// Passed to every load, so destroying one document's task leaves the worker to
// the next; a task that made its own would tear it down with it.
const worker = PDFWorker.create({ port: new PdfWorker() });

// Versioned because the server caches these unhashed files as immutable; see
// the pdfjs plugin in vite.config.ts.
const ASSETS = `/pdfjs/${version}/`;

type FindMatches = { current: number; total: number };

/** Every webview gets PDF.js: WebKitGTK has no PDF viewer of its own. */
export default function PdfPreview({
  url,
  name,
  onError,
}: {
  url: string;
  name: string;
  onError: () => void;
}) {
  const paneRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const findInputRef = useRef<HTMLInputElement>(null);
  const viewerRef = useRef<PDFViewer | null>(null);
  const eventBusRef = useRef<EventBus | null>(null);
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;
  const [page, setPage] = useState(1);
  const [pageCount, setPageCount] = useState(0);
  const [query, setQuery] = useState("");
  const [matches, setMatches] = useState<FindMatches | null>(null);

  useEffect(() => {
    const pane = paneRef.current;
    const container = containerRef.current;
    if (!pane || !container) return;
    const eventBus = new EventBus();
    const linkService = new PDFLinkService({
      eventBus,
      externalLinkTarget: LinkTarget.BLANK,
      externalLinkRel: "noopener noreferrer",
    });
    // One count once the scan finishes, not one per page for the live region.
    const findController = new PDFFindController({
      eventBus,
      linkService,
      updateMatchesCountOnProgress: false,
    });
    const listeners = new AbortController();
    // A variable, not a literal: the typings omit `abortSignal`, which tears down
    // the viewer's own scroll listener and ResizeObserver.
    const options = {
      container,
      eventBus,
      linkService,
      findController,
      abortSignal: listeners.signal,
    };
    const viewer = new PDFViewer(options);
    linkService.setViewer(viewer);
    eventBus.on("pagesinit", () => {
      viewer.currentScaleValue = "page-width";
    });
    eventBus.on("pagechanging", ({ pageNumber }: { pageNumber: number }) => setPage(pageNumber));
    const onMatches = ({ matchesCount }: { matchesCount: FindMatches }) => setMatches(matchesCount);
    eventBus.on("updatefindmatchescount", onMatches);
    eventBus.on("updatefindcontrolstate", onMatches);
    viewerRef.current = viewer;
    eventBusRef.current = eventBus;

    // Page-width is a fit, not a size: keep fitting as the pane resizes. The
    // pane, not the scroller, whose width changes as its scrollbar comes and goes.
    const resize = new ResizeObserver(() => {
      if (viewer.currentScaleValue === "page-width") viewer.currentScaleValue = "page-width";
    });
    resize.observe(pane);

    const task = getDocument({
      url,
      worker,
      cMapUrl: `${ASSETS}cmaps/`,
      standardFontDataUrl: `${ASSETS}standard_fonts/`,
      wasmUrl: `${ASSETS}wasm/`,
      iccUrl: `${ASSETS}iccs/`,
    });
    let cancelled = false;
    task.promise.then(
      (pdf) => {
        if (cancelled) return;
        // The viewer hands the document to the find controller itself.
        viewer.setDocument(pdf);
        linkService.setDocument(pdf);
        setPageCount(pdf.numPages);
      },
      () => {
        if (!cancelled) onErrorRef.current();
      },
    );
    return () => {
      cancelled = true;
      resize.disconnect();
      // Cancels rendering and releases the text layers, which PDF.js otherwise
      // keeps in a module-level map. The typings don't admit null.
      Reflect.apply(viewer.setDocument, viewer, [null]);
      listeners.abort();
      void task.destroy();
      viewerRef.current = null;
      eventBusRef.current = null;
    };
  }, [url]);

  function find(text: string, again: boolean, previous = false) {
    eventBusRef.current?.dispatch("find", {
      source: null,
      type: again ? "again" : "",
      query: text,
      caseSensitive: false,
      entireWord: false,
      highlightAll: true,
      findPrevious: previous,
      matchDiacritics: false,
    });
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    // The page text is split across positioned spans, which the webview's own
    // find doesn't search as one document.
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "f") {
      event.preventDefault();
      findInputRef.current?.focus();
      findInputRef.current?.select();
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col" onKeyDown={onKeyDown}>
      <div className="flex shrink-0 items-center gap-1 border-b border-border-variant py-1 px-4">
        <span className="px-1 text-xs text-subtext tabular-nums whitespace-nowrap">
          {pageCount ? `${fmtNumber(page)} / ${fmtNumber(pageCount)}` : ""}
        </span>
        <IconButton
          size="small"
          aria-label={m.image_zoom_out()}
          data-tip={m.image_zoom_out()}
          onClick={() => viewerRef.current?.decreaseScale()}
        >
          <Minus size={13} />
        </IconButton>
        <IconButton
          size="small"
          aria-label={m.image_zoom_in()}
          data-tip={m.image_zoom_in()}
          onClick={() => viewerRef.current?.increaseScale()}
        >
          <Plus size={13} />
        </IconButton>
        <IconButton
          size="small"
          aria-label={m.pdf_preview_fit_width()}
          data-tip={m.pdf_preview_fit_width()}
          onClick={() => {
            if (viewerRef.current) viewerRef.current.currentScaleValue = "page-width";
          }}
        >
          <MoveHorizontal size={13} />
        </IconButton>
        <div className="ms-auto flex min-w-0 items-center gap-1">
          <Input
            ref={findInputRef}
            className="h-7 w-48 min-w-0 shrink"
            type="search"
            placeholder={m.pdf_preview_find()}
            aria-label={m.pdf_preview_find()}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              // The new count arrives only once the whole document is scanned.
              setMatches(null);
              find(event.target.value, false);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                find(query, true, event.shiftKey);
              }
            }}
          />
          <span className="px-1 text-xs text-subtext tabular-nums whitespace-nowrap" aria-live="polite">
            {query && matches
              ? matches.total
                ? `${fmtNumber(matches.current)} / ${fmtNumber(matches.total)}`
                : fmtNumber(0)
              : ""}
          </span>
          <IconButton
            size="small"
            aria-label={m.pdf_preview_previous_match()}
            data-tip={m.pdf_preview_previous_match()}
            disabled={!query}
            onClick={() => find(query, true, true)}
          >
            <ChevronUp size={13} />
          </IconButton>
          <IconButton
            size="small"
            aria-label={m.pdf_preview_next_match()}
            data-tip={m.pdf_preview_next_match()}
            data-tip-align="end"
            disabled={!query}
            onClick={() => find(query, true)}
          >
            <ChevronDown size={13} />
          </IconButton>
        </div>
      </div>
      {/* PDFViewer requires an absolutely positioned scroll container. */}
      <div ref={paneRef} className="relative min-h-0 flex-1 bg-surface">
        {/* Focusable, so a click in the pages keeps Cmd/Ctrl+F here and the keys scroll. */}
        <div
          ref={containerRef}
          role="document"
          aria-label={name}
          tabIndex={0}
          className="absolute inset-0 overflow-auto focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-text focus-visible:-outline-offset-2"
        >
          <div className="pdfViewer" />
        </div>
      </div>
    </div>
  );
}
