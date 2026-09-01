import { useEffect, useState } from "react";
import { m } from "../paraglide/messages.js";
import { ltr } from "../i18n";
import { isExternalMarkdownTarget } from "../markdownTarget";
import { Spinner } from "./ui";

/** Elements whose relative URLs name a project file, each with the response
 * types it may legitimately be. The gate is the security boundary: the parent
 * can read every file the backend serves, so without it a document could name a
 * key file and have its own scripts read back the inlined bytes. It rests on the
 * server typing a response by its resolved path, not by the name asked for. */
const INLINABLE: { selector: string; attribute: string; typePrefixes: string[] }[] = [
  { selector: "img[src]", attribute: "src", typePrefixes: ["image/"] },
  { selector: "source[src]", attribute: "src", typePrefixes: ["image/", "audio/", "video/"] },
  { selector: "video[poster]", attribute: "poster", typePrefixes: ["image/"] },
  { selector: "video[src]", attribute: "src", typePrefixes: ["video/"] },
  { selector: "audio[src]", attribute: "src", typePrefixes: ["audio/"] },
  { selector: 'link[rel~="stylesheet"][href]', attribute: "href", typePrefixes: ["text/css"] },
  { selector: "script[src]", attribute: "src", typePrefixes: ["text/javascript"] },
];
/** Total inlined bytes, and how many files are worth fetching at all. */
const MAX_INLINE_BYTES = 4_000_000;
const MAX_INLINE_FILES = 200;
/** Ceiling for the refetch of a body the read cap truncated. */
const MAX_DOCUMENT_BYTES = 16_000_000;

const asDataUrl = (blob: Blob) =>
  new Promise<string | null>((resolve) => {
    const reader = new FileReader();
    reader.onload = () => resolve(typeof reader.result === "string" ? reader.result : null);
    reader.onerror = () => resolve(null);
    reader.readAsDataURL(blob);
  });

/** `//cdn/x.js` carries no scheme, and the frame's base has none to lend it. */
const withScheme = (value: string) => (value.startsWith("//") ? `https:${value}` : value);

type Asset = { element: Element; attribute: string; url: string; typePrefixes: string[] };

/** Replace each project-local asset with its bytes. The frame renders in an
 * opaque origin, which this browser refuses to let reach a loopback URL — a
 * rewritten URL never even leaves — so the bytes have to travel in the document.
 * Sequential, so the budget spends in the order they were collected rather than
 * by whichever response lands first. */
async function inlineAssets(assets: Asset[], signal: AbortSignal) {
  let budget = MAX_INLINE_BYTES;
  // Keyed by URL, `null` for one that didn't land, so a file costs one attempt
  // however many times the document names it. Past the cap the rest still get
  // whatever is already inlined.
  const inlined = new Map<string, string | null>();
  for (const { element, attribute, url, typePrefixes } of assets) {
    if (inlined.has(url)) {
      const cached = inlined.get(url);
      if (cached) element.setAttribute(attribute, cached);
      continue;
    }
    if (signal.aborted) return;
    if (inlined.size >= MAX_INLINE_FILES) continue;
    inlined.set(url, null);
    const response = await fetch(url, { signal }).catch(() => null);
    if (!response?.ok) continue;
    const type = response.headers.get("content-type") ?? "";
    const size = Number(response.headers.get("content-length"));
    if (
      !typePrefixes.some((prefix) => type.startsWith(prefix)) ||
      !(Number.isFinite(size) && size > 0 && size <= budget)
    ) {
      await response.body?.cancel().catch(() => {});
      continue;
    }
    const blob = await response.blob().catch(() => null);
    const dataUrl = blob && (await asDataUrl(blob));
    if (!blob || !dataUrl) continue;
    budget -= blob.size;
    inlined.set(url, dataUrl);
    element.setAttribute(attribute, dataUrl);
  }
}

/** The document as the frame should receive it. Resolves what it can reach —
 * the elements in `INLINABLE` only, so `srcset`, CSS `url()` and an inlined
 * stylesheet's own references stay unresolved — and defuses the rest. */
async function assembleDocument(
  html: string,
  resolveSrc: (src: string) => string | null,
  signal: AbortSignal,
): Promise<string> {
  const doc = new DOMParser().parseFromString(html, "text/html");

  // One pass over the document so the budget really does spend in document
  // order: grouping by rule would starve the stylesheet behind the figures.
  const assets: Asset[] = [];
  for (const element of doc.querySelectorAll(INLINABLE.map((rule) => rule.selector).join(", "))) {
    for (const { selector, attribute, typePrefixes } of INLINABLE) {
      if (!element.matches(selector)) continue;
      const value = element.getAttribute(attribute);
      if (!value) continue;
      const url = resolveSrc(value);
      if (!url) continue;
      // Unchanged means external (a CDN script, an https image): the frame
      // fetches that for itself.
      if (url === value) element.setAttribute(attribute, withScheme(value));
      else assets.push({ element, attribute, url, typePrefixes });
    }
  }
  await inlineAssets(assets, signal);

  // Followed in place, an outbound link turns the pane into a chromeless browser
  // with no way back.
  for (const anchor of doc.querySelectorAll("a[href]")) {
    const href = anchor.getAttribute("href");
    if (!href || !isExternalMarkdownTarget(href)) continue;
    anchor.setAttribute("href", withScheme(href));
    anchor.setAttribute("target", "_blank");
    anchor.setAttribute("rel", "noopener noreferrer");
  }

  // Left to itself a `srcdoc` document inherits the dashboard's base URL, so a
  // report's `#section` link navigates the frame into the SPA and the page is
  // gone. `about:` can't be a base, so what this pass missed resolves to nothing
  // instead. A document's own base counts only when it names an origin — a bare
  // `<base href="/">` would hand it the dashboard's back.
  const authored = doc.querySelector("base[href]")?.getAttribute("href") ?? "";
  if (!/^https?:\/\//i.test(authored)) {
    const base = doc.createElement("base");
    base.setAttribute("href", "about:srcdoc");
    doc.head.prepend(base);
  }
  // `outerHTML` drops the doctype, and a page without one renders in quirks
  // mode. The name alone: no generated report turns on a legacy identifier.
  return `${doc.doctype ? `<!DOCTYPE ${doc.doctype.name}>` : ""}${doc.documentElement.outerHTML}`;
}

/** The whole file: the loaded body, or a refetch of it when the read cap
 * truncated it — a self-contained plot easily runs past 512 KB, and half a
 * document renders as a page that just stops. */
async function completeSource(
  html: string,
  truncated: boolean,
  url: string,
  signal: AbortSignal,
): Promise<{ text: string; partial: boolean }> {
  if (!truncated) return { text: html, partial: false };
  const response = await fetch(url, {
    signal,
    headers: { Range: `bytes=0-${MAX_DOCUMENT_BYTES - 1}` },
  }).catch(() => null);
  const body = response?.ok ? await response.text().catch(() => null) : null;
  // Nothing else serves this file, so a failed refetch still shows the capped
  // body — labelled, rather than passing off a page that stops as the whole one.
  if (body === null) return { text: html, partial: true };
  const total = Number(response?.headers.get("content-range")?.split("/").pop());
  return { text: body, partial: Number.isFinite(total) && total > MAX_DOCUMENT_BYTES };
}

/** An HTML file rendered as the page it is, in a frame with no access to the
 * dashboard: its scripts run in an opaque origin, so a generated report can
 * draw itself without reaching the API, this page, or the browser's storage. */
export function HtmlPreview({
  html,
  truncated,
  url,
  name,
  resolveSrc,
}: {
  html: string;
  truncated: boolean;
  /** Raw bytes of this file, for the refetch a truncated body needs. */
  url: string;
  /** Names the frame for a screen reader rotoring between open tabs. */
  name: string;
  resolveSrc: (src: string) => string | null;
}) {
  const [page, setPage] = useState<{ source: string; partial: boolean } | null>(null);

  useEffect(() => {
    let cancelled = false;
    const controller = new AbortController();
    setPage(null);
    completeSource(html, truncated, url, controller.signal)
      .then(async ({ text, partial }) => ({
        source: await assembleDocument(text, resolveSrc, controller.signal),
        partial,
      }))
      .then((next) => {
        if (!cancelled) setPage(next);
      });
    return () => {
      cancelled = true;
      controller.abort();
    };
  }, [html, truncated, url, resolveSrc]);

  if (page === null) {
    return (
      <div className="file-view-note flex items-center gap-2 py-2.5 px-4 text-sm text-muted">
        <Spinner /> {m.file_viewer_loading()}
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      {page.partial && (
        <div className="file-view-note shrink-0 border-b border-b-border-variant py-2 px-4 text-sm text-muted">
          {m.file_viewer_html_partial()}
        </div>
      )}
      <iframe
        // White-backed: an unstyled document assumes the browser default, and
        // the pane's dark ground would leave its black text unreadable.
        className="block min-h-0 flex-1 w-full border-0 bg-white"
        title={m.file_viewer_html_preview({ name: ltr(name) })}
        // Never allow-same-origin: it would hand a repo's or an agent's HTML
        // this page's origin and the unauthenticated loopback API. Popups so a
        // report's links open (they inherit the sandbox, having no
        // allow-popups-to-escape-sandbox), downloads so plot toolbars save.
        sandbox="allow-scripts allow-popups allow-downloads"
        referrerPolicy="no-referrer"
        srcDoc={page.source}
     />
    </div>
  );
}
