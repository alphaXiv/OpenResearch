// Brand marks + command parsing for the literature sources, used to render
// `orx lit` / `orx paper` tool calls in chat as a real search ("Searching
// OpenAlex for …") instead of a raw shell line. The official SVGs are inlined
// at build time via `?raw` (no external asset — the UI is rust-embedded and
// CSP-locked) and shown in a small white tile so the black marks (OpenAlex,
// bioRxiv) stay visible in dark mode and every source reads uniformly.

import alphaxivSvg from "../assets/lit-sources/alphaxiv.svg?raw";
import biorxivSvg from "../assets/lit-sources/biorxiv.svg?raw";
import openalexSvg from "../assets/lit-sources/openalex.svg?raw";

export type LitSource = "alphaxiv" | "openalex" | "biorxiv";

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
}: {
  source: LitSource;
  size?: number;
  decorative?: boolean;
}) {
  return (
    <span
      className="lit-logo flex-none inline-flex items-center justify-center p-[1.5px] box-border bg-white rounded-[3px] shadow-[0_0_0_1px_rgba(0,_0,_0,_0.08)] [&_svg]:w-full [&_svg]:h-full [&_svg]:block"
      style={{ width: size, height: size }}
      {...(decorative ? { "aria-hidden": true } : { role: "img", "aria-label": LIT_SOURCE_NAME[source] })}
      // Static, build-inlined brand SVGs — not user input.
      dangerouslySetInnerHTML={{ __html: LIT_SOURCE_SVG[source] }}
    />
  );
}

export type OrxLitCall =
  | { kind: "lit"; source: LitSource; query?: string }
  | { kind: "paper"; source: LitSource; id?: string };

function asSource(v: string | undefined): LitSource | undefined {
  return v === "alphaxiv" || v === "openalex" || v === "biorxiv" ? v : undefined;
}

/** Mirror of the Rust `detect_source` used by `orx paper` when no `--source`
 * is given: host hints first, then a `10.1101/…` DOI → biorxiv, any other
 * `10.…/…` DOI or a bare `W…` id → openalex, else alphaXiv. */
function detectPaperSource(id: string): LitSource {
  const s = id.trim();
  const lower = s.toLowerCase();
  if (lower.includes("biorxiv.org")) return "biorxiv";
  if (lower.includes("openalex.org")) return "openalex";
  // A real DOI is `10.<registrant>/<suffix>` — the slash distinguishes it from
  // an arXiv id like `2410.12345` (October) that also contains "10.".
  const doi = s.match(/10\.\d+\/\S+/);
  if (doi) return doi[0].startsWith("10.1101/") ? "biorxiv" : "openalex";
  const last = s.split("/").pop() ?? "";
  if (/^W\d+$/i.test(last)) return "openalex";
  return "alphaxiv";
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

/** Tokenize the args after `orx lit`/`orx paper`, respecting quotes and
 * stopping at a shell operator (`|`, `;`, `>`, `&`). */
function tokenizeArgs(s: string): string[] {
  const tokens: string[] = [];
  let cur = "";
  let has = false;
  let quote: '"' | "'" | null = null;
  for (const c of s) {
    if (quote) {
      if (c === quote) quote = null;
      else cur += c;
      has = true;
      continue;
    }
    if (c === '"' || c === "'") {
      quote = c;
      has = true;
      continue;
    }
    if (c === " " || c === "\t" || c === "\n") {
      if (has) {
        tokens.push(cur);
        cur = "";
        has = false;
      }
      continue;
    }
    if (c === "|" || c === ";" || c === ">" || c === "&") break;
    cur += c;
    has = true;
  }
  if (has) tokens.push(cur);
  return tokens;
}

/** Recognize an `orx lit` / `orx paper` invocation inside a shell command and
 * pull out the source + query/id. Returns null for anything else. */
export function parseOrxLit(command: string): OrxLitCall | null {
  const m = command.match(/(?:^|[\s;&|(])orx\s+(lit|paper)\b/);
  if (!m) return null;
  const kind = m[1] as "lit" | "paper";
  const rest = command.slice((m.index ?? 0) + m[0].length);
  const tokens = tokenizeArgs(rest);

  let source: LitSource | undefined;
  let positional: string | undefined;
  for (let i = 0; i < tokens.length; i++) {
    const t = tokens[i];
    if (t === "--source") {
      source = asSource(tokens[++i]);
      continue;
    }
    if (t.startsWith("--source=")) {
      source = asSource(t.slice("--source=".length));
      continue;
    }
    // `--limit` takes a value; skip it so the value isn't read as the query.
    if (t === "--limit") {
      i++;
      continue;
    }
    if (t.startsWith("--")) continue; // --json / --full / --limit=N
    if (positional === undefined && t) positional = t;
  }

  if (kind === "lit") {
    return { kind, source: source ?? "alphaxiv", query: positional };
  }
  return {
    kind,
    source: source ?? (positional ? detectPaperSource(positional) : "alphaxiv"),
    id: positional,
  };
}
