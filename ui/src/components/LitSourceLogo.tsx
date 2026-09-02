// Brand marks + command parsing for the literature sources, used to render
// Discovery and `orx paper` tool calls in chat as a real search ("Searching
// OpenAlex for …") instead of a raw shell line. The official SVGs are inlined
// at build time via `?raw` (no external asset — the UI is rust-embedded and
// CSP-locked) and shown in a small white tile so the black marks (OpenAlex,
// bioRxiv) stay visible in dark mode and every source reads uniformly.

import alphaxivSvg from "../assets/lit-sources/alphaxiv.svg?raw";
import biorxivSvg from "../assets/lit-sources/biorxiv.svg?raw";
import openalexSvg from "../assets/lit-sources/openalex.svg?raw";
import type { LitSource } from "../orxCommand";
export { parseOrxLit, type LitSource, type OrxLitCall } from "../orxCommand";

export const LIT_SOURCE_NAME: Record<LitSource, string> = {
  alphaxiv: "alphaXiv",
  openalex: "OpenAlex",
  biorxiv: "bioRxiv",
};

const LIT_SOURCE_SVG: Record<LitSource, string> = {
  alphaxiv: alphaxivSvg,
  openalex: openalexSvg,
  biorxiv: biorxivSvg,
};

/** `decorative` when the source name is already shown as adjacent text (Settings
 * rows); otherwise the logo carries the source name for screen readers. */
export function LitSourceLogo({
  source,
  size = 16,
  decorative = false,
  className = "",
}: {
  source: LitSource;
  size?: number;
  decorative?: boolean;
  className?: string;
}) {
  return (
    <span
      className={`lit-logo flex-none inline-flex items-center justify-center p-[1.5px] box-border bg-white rounded-[3px] shadow-logo [&_svg]:w-full [&_svg]:h-full [&_svg]:block ${className}`}
      style={{ width: size, height: size }}
      {...(decorative ? { "aria-hidden": true } : { role: "img", "aria-label": LIT_SOURCE_NAME[source] })}
      // Static, build-inlined brand SVGs — not user input.
      dangerouslySetInnerHTML={{ __html: LIT_SOURCE_SVG[source] }}
   />
  );
}

/** Bare DOI (`10.<registrant>/<suffix>`) out of an id or URL, or null. Strips a
 * trailing bioRxiv content-URL suffix (`v2`, `v2.full`, `v2.full.pdf`) so the DOI
 * resolves on doi.org. */
function doiFrom(id: string): string | null {
  const s = id.trim().replace(/^https?:\/\/doi\.org\//i, "").replace(/^doi:/i, "");
  const m = s.match(/10\.\d+\/[^\s?#]+/);
  if (!m) return null;
  return m[0].replace(/[.,)]+$/, "").replace(/v\d+(\.[a-z][a-z-]*)*$/i, "");
}

/** The public page to open for a fetched paper, on its own source: alphaXiv for
 * arXiv ids, the resolving DOI (→ bioRxiv) for bioRxiv, and the DOI or the
 * OpenAlex work page for OpenAlex. */
export function paperUrl(source: LitSource, id: string): string {
  const s = id.trim();
  if (source === "alphaxiv") {
    const last = s.split(/[?#]/)[0].split("/").pop() || s;
    const arxivId = last.replace(/\.(pdf|md)$/i, "");
    return `https://www.alphaxiv.org/abs/${encodeURIComponent(arxivId)}`;
  }
  const doi = doiFrom(s);
  if (doi) return `https://doi.org/${doi}`;
  if (source === "openalex") {
    const wid = s.split("/").pop() || s;
    return `https://openalex.org/${encodeURIComponent(wid)}`;
  }
  return `https://doi.org/${s}`;
}
