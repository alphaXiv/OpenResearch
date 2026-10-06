import { m } from "../paraglide/messages.js";
// A mermaid block as a diagram, with its source always one click away.
//
// The engine loads on first mount of a diagram (see ../mermaid.ts), so a
// document containing no diagrams never fetches it. Everything that can go
// wrong — source mermaid cannot parse, a diagram type this build does not
// ship, an import that failed — lands on the same path: highlighted source
// plus the reason, so a diagram that will not draw is never a blank box.

import { Check, Code, Copy, Workflow } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { highlight } from "../syntaxHighlight";
import { renderMermaid, type MermaidRenderResult } from "../mermaid";
import { IconButton } from "./ui";

// Chat blocks are short; cap tokenizing well below the file viewer's limit.
const HIGHLIGHT_MAX_BYTES = 100_000;

/** How long a revised diagram's source must hold still before it is re-drawn. */
const STREAM_SETTLE_MS = 150;

/**
 * Copies the diagram source, falling back to selecting it when the clipboard
 * write is refused. `fallbackRef` must point at the element holding that text:
 * selecting document.body would hand the user the whole dashboard to copy.
 */
function CopyButton({ code, fallbackRef }: { code: string; fallbackRef?: RefObject<HTMLElement | null> }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      const target = fallbackRef?.current;
      if (!target) return;
      const range = document.createRange();
      range.selectNodeContents(target);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
    }
  };
  return (
    <IconButton
      size="small"
      className="bg-background"
      title={m.md_copy()}
      aria-label={m.md_copy_code()}
      onClick={() => void copy()}
    >
      {copied ? <Check size={13} /> : <Copy size={13} />}
    </IconButton>
  );
}

export function MermaidDiagram({ code }: { code: string }) {
  const [result, setResult] = useState<MermaidRenderResult | null>(null);
  const [showDiagram, setShowDiagram] = useState(true);
  // Re-render on a theme flip: mermaid bakes its palette into the SVG, so the
  // markup goes stale the moment <html data-theme> changes.
  const [theme, setTheme] = useState(() => document.documentElement.dataset.theme);
  const hostRef = useRef<HTMLDivElement>(null);
  const sourceRef = useRef<HTMLPreElement>(null);

  useEffect(() => {
    const observer = new MutationObserver(() =>
      setTheme(document.documentElement.dataset.theme),
    );
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    return () => observer.disconnect();
  }, []);

  // A file preview renders its diagram immediately. A chat transcript does not:
  // there, `code` grows with every token, and re-running the engine's layout on
  // each one would cost more than it is worth. Later revisions wait for the
  // burst to settle, and keep the previous diagram on screen meanwhile so a
  // growing diagram does not flash to source and back on every token.
  const settled = useRef(false);
  useEffect(() => {
    let live = true;
    const apply = (next: MermaidRenderResult) => {
      if (live) setResult(next);
    };
    if (!settled.current) {
      settled.current = true;
      void renderMermaid(code).then(apply);
      return () => {
        live = false;
      };
    }
    const timer = window.setTimeout(() => {
      void renderMermaid(code).then(apply);
    }, STREAM_SETTLE_MS);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [code, theme]);

  // Mermaid hands interactivity back separately from the markup, so bind it to
  // the node that is actually in the document once that markup lands. Toggling
  // to source and back unmounts the host, so the next mount is a brand new svg
  // carrying none of the previous handlers — hence showDiagram in the deps, not
  // just result.
  useLayoutEffect(() => {
    const svg = hostRef.current?.querySelector("svg");
    if (svg && result?.ok) result.bindFunctions?.(svg);
  }, [result, showDiagram]);

  // A failed or still-pending render has no diagram to go back to, so the
  // toggle only appears once there is something to toggle.
  const rendered = result?.ok === true;
  const reason = result && !result.ok ? result.error : null;

  return (
    <div className="mermaid-diagram md-code relative my-2.5 mx-0 [&_pre]:m-0">
      <div className="absolute top-1.5 end-1.5 z-1 flex gap-1">
        {rendered && (
          <IconButton
            size="small"
            className="bg-background"
            active={!showDiagram}
            data-tip={showDiagram ? m.mermaid_show_source() : m.mermaid_show_diagram()}
            aria-label={showDiagram ? m.mermaid_show_source() : m.mermaid_show_diagram()}
            onClick={() => setShowDiagram((value) => !value)}
          >
            {showDiagram ? <Code size={13} /> : <Workflow size={13} />}
          </IconButton>
        )}
        <CopyButton code={code} fallbackRef={sourceRef} />
      </div>

      {result?.ok && showDiagram ? (
        <div
          ref={hostRef}
          className="mermaid-diagram-canvas overflow-auto border border-border-muted rounded-md bg-surface p-3"
          // Markup from the engine, not from the document: it is laid out from
          // source mermaid has just parsed, and the engine is configured to
          // emit SVG <text> labels rather than HTML, so there is no authored
          // markup in here to inject (see ../mermaid.ts).
          dangerouslySetInnerHTML={{ __html: result.svg }}
        />
      ) : (
        <>
          {/* A standalone .mmd file is rendered outside any markdown container,
              which is where the prose styles for <pre> live, so the code block
              carries the equivalent utilities itself. */}
          <pre ref={sourceRef} className="mermaid-diagram-source m-0 overflow-x-auto rounded-md border border-border-muted bg-surface py-2 px-3 text-sm text-text [&_code]:border-0 [&_code]:bg-transparent [&_code]:p-0 [&_code]:font-mono [&_code]:text-inherit">
            <code>{highlight(code, "mermaid", HIGHLIGHT_MAX_BYTES)}</code>
          </pre>
          {reason && (
            <p className="mermaid-diagram-fallback m-0 py-1.5 pe-2 ps-2.5 text-xs text-subtext">
              {reason === "unsupported"
                ? m.mermaid_not_a_diagram()
                : m.mermaid_could_not_render({ error: reason })}
            </p>
          )}
        </>
      )}
    </div>
  );
}