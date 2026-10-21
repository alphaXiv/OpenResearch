// Mermaid is the heaviest optional thing the dashboard can draw, so it is
// imported lazily and cached: nothing is fetched until a document actually
// contains a diagram, and the engine is paid for at most once per session.
//
// Full `mermaid` rather than `@mermaid-js/tiny`, deliberately. Measured on this
// app's Vite build, gzip, for a flowchart as the first diagram in a document:
//
//   @mermaid-js/tiny 12.1.0              743 kB, one file, no lazy loading
//   mermaid 12.1.0, ELK layout (default)  625 kB  (163 core + 441 elk + 20 flow)
//   mermaid 12.1.0, dagre layout          200 kB  (163 core +  16 dagre + 20 flow)
//
// So dagre is set as the default layout below and ELK is left to the diagrams
// that ask for it by directive, which is both the cheapest common case and the
// only one that reaches for a 441 kB engine. Nothing is fetched until a
// document contains a diagram, so the start-up path is untouched either way.
//
// Tiny loses on every axis that matters here: 3.7x the on-demand cost, no
// lazy loading, no mindmap or architecture diagrams, and its ELK requests fall
// back to dagre anyway — it excludes elkjs from the bundle, so a diagram that
// names an ELK layout silently loses it rather than saying so.

/** Fence info strings that mean "this block is a mermaid diagram". */
const MERMAID_LANGUAGES = new Set(["mermaid"]);

/** Extensions a standalone diagram file can use. */
const MERMAID_FILE_RE = /\.(mmd|mermaid)$/i;

/** True for a fenced block whose info string marks it as a mermaid diagram. */
export function isMermaidLanguage(language: string | null | undefined): boolean {
  return language != null && MERMAID_LANGUAGES.has(language.trim().toLowerCase());
}

/** True for a standalone diagram file (`.mmd`, `.mermaid`). */
export function isMermaidFile(name: string): boolean {
  return MERMAID_FILE_RE.test(name);
}

/**
 * The first meaningful line, which is where mermaid's diagram declaration
 * lives. `%%` starts a mermaid comment, so leading comment lines are skipped.
 */
export function mermaidDeclarationLine(code: string): string {
  for (const line of code.split("\n")) {
    const trimmed = line.trim();
    if (trimmed === "" || trimmed.startsWith("%%")) continue;
    return trimmed;
  }
  return "";
}

/**
 * Whether a block holds anything a diagram could be drawn from. This is the
 * only question worth answering before the engine is fetched: mermaid picks a
 * parser from per-diagram detectors it registers at runtime, so guessing the
 * type here would mean freezing its keyword list. Anything that survives this
 * gate goes to mermaid and is judged by its own parser, which is also what
 * makes an unsupported diagram type fall back to source instead of failing.
 */
export function mermaidHasContent(code: string): boolean {
  return mermaidDeclarationLine(code) !== "";
}

/** Mermaid's palette choice, following the app's own `<html data-theme>`. */
export function mermaidTheme(): "default" | "dark" {
  return document.documentElement.dataset.theme === "dark" ? "dark" : "default";
}

/** Read the app font rather than hardcoding one, so labels match the chrome. */
function appFontFamily(): string {
  const family = getComputedStyle(document.body).fontFamily;
  return family || "system-ui, sans-serif";
}

export interface MermaidRenderOk {
  ok: true;
  /** Inline SVG markup, with mermaid's own sizing and defs intact. */
  svg: string;
  /**
   * Mermaid attaches interactivity (click handlers, node toggles) separately
   * from the markup, so the caller must invoke this on the element it inserted.
   */
  bindFunctions: ((element: Element) => void) | undefined;
}

export interface MermaidRenderFailed {
  ok: false;
  /** Human-readable reason, shown next to the source fallback. */
  error: string;
}

export type MermaidRenderResult = MermaidRenderOk | MermaidRenderFailed;

interface MermaidRuntime {
  initialize: (config: Record<string, unknown>) => void;
  render: (
    id: string,
    text: string,
    container?: Element,
  ) => Promise<{ svg: string; bindFunctions?: (element: Element) => void }>;
  parseError?: unknown;
}

let runtime: Promise<MermaidRuntime> | null = null;
/** Mermaid derives generated DOM ids from this, so ids must never repeat. */
let renderSeq = 0;
/** The theme the engine was last configured for, so initialize runs once. */
let configuredTheme: string | null = null;

function loadRuntime(): Promise<MermaidRuntime> {
  if (!runtime) {
    // A rejected import must not be cached: the chunk can fail to arrive for
    // reasons that clear (a transient read error, a half-written update), and
    // holding the rejection would leave every diagram in source fallback for
    // the rest of the session. Clearing on failure makes the next render retry.
    const pending = import("mermaid").then((mod) => {
      const mermaid = mod.default as unknown as MermaidRuntime;
      // Clearing parseError makes an unparseable diagram *throw* instead of
      // painting mermaid's own error box into the document. The caller shows
      // the source instead, which is more use than a red error diagram.
      mermaid.parseError = undefined;
      return mermaid;
    });
    runtime = pending;
    void pending.catch(() => {
      if (runtime === pending) runtime = null;
    });
  }
  return runtime;
}

/**
 * `initialize` is global to the engine, so it is driven from here rather than
 * left to callers. The theme is the only part that moves — it follows the app's
 * `<html data-theme>` — so this runs once per theme rather than once per
 * diagram, leaving initialize → many renders the shape mermaid expects.
 *
 * `htmlLabels: false` is load-bearing, not a preference. Labels default to a
 * `foreignObject` holding real HTML, and `securityLevel: "strict"` sanitizes
 * that HTML through DOMPurify — whose allow-list still permits `<img src>`. A
 * diagram in a project file (or in model output) can therefore name an
 * arbitrary host in a label and have the app fetch it once the SVG is inserted.
 * SVG `<text>` labels remove the HTML surface instead of filtering it, which is
 * the same trade GitHub's renderer makes. `strict` is kept as well so the
 * engine's own output is sanitized regardless.
 */
function ensureConfigured(mermaid: MermaidRuntime): void {
  const theme = mermaidTheme();
  if (configuredTheme === theme) return;
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    suppressErrorRendering: true,
    htmlLabels: false,
    // Keys listed here cannot be overridden by a diagram's own
    // `%%{init: {...}}%%` directive. Without this, htmlLabels above is only a
    // default: any .mmd file or markdown fence can set it back to true, which
    // restores the foreignObject label path — and DOMPurify permits <img src>,
    // so an author-chosen URL is fetched when the SVG is inserted. Verified:
    // before this, a diagram carrying `%%{init: {"htmlLabels": true}}%%` and an
    // <img> label did issue the request. Keep both the default and this list.
    secure: ["htmlLabels", "securityLevel", "startOnLoad", "suppressErrorRendering", "maxTextSize", "maxEdges"],
    // mermaid defaults to ELK, whose engine is 441 kB gzip of its own — more
    // than the rest of the diagram put together. Dagre is a 16 kB chunk and
    // lays out the flowcharts, sequence and state diagrams that dominate this
    // app's output. A diagram can still ask for ELK, because a per-diagram
    // `%%{init: {"layout": "elk"}}%%` directive overrides this default, and
    // only then does the engine load.
    layout: "dagre",
    theme,
    fontFamily: appFontFamily(),
  });
  configuredTheme = theme;
}

/**
 * Render a diagram to SVG markup. Never throws: malformed source, an
 * unsupported diagram type, or an engine that failed to load all come back as
 * `ok: false` for the caller to answer with source.
 */
export async function renderMermaid(code: string): Promise<MermaidRenderResult> {
  const source = code.trim();
  if (!source || !mermaidHasContent(source)) {
    return { ok: false, error: "unsupported" };
  }

  let mermaid: MermaidRuntime;
  try {
    mermaid = await loadRuntime();
    ensureConfigured(mermaid);
  } catch (error) {
    return { ok: false, error: describe(error) };
  }

  try {
    const { svg, bindFunctions } = await mermaid.render(`orx-mermaid-${renderSeq++}`, source);
    return { ok: true, svg, bindFunctions };
  } catch (error) {
    return { ok: false, error: describe(error) };
  }
}

function describe(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  return typeof error === "string" && error ? error : "render-failed";
}