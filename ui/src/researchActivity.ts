import type { TrackioMetric } from "./api";
import type { LitSource } from "./orxCommand";

export interface PaperMeta {
  title?: string;
  authors?: string[];
  year?: number;
}

const DISCOVERED_PAPER = /"id":\s*"((?:[^"\\]|\\.)*)",\s*"title":\s*"((?:[^"\\]|\\.)*)"/g;

function jsonString(escaped: string): string | undefined {
  try {
    return JSON.parse(`"${escaped}"`);
  } catch {
    return undefined;
  }
}

/** Titles and years of the papers `orx discover` printed, keyed by id. Reads
 * output a harness cut short, so it matches hits one by one. */
export function discoveredPapers(outputs: string[]): Map<string, PaperMeta> {
  const papers = new Map<string, PaperMeta>();
  for (const output of outputs) {
    for (const match of output.matchAll(DISCOVERED_PAPER)) {
      const id = jsonString(match[1]);
      const title = jsonString(match[2]);
      if (!id || !title || papers.has(id)) continue;
      const rest = output.slice(match.index + match[0].length, match.index + match[0].length + 20_000);
      const next = rest.search(/"id":\s*"/);
      const year = /"publicationDate":\s*"(\d{4})/.exec(next < 0 ? rest : rest.slice(0, next))?.[1];
      papers.set(id, { title: title.replace(/\s+/g, " ").trim(), year: year ? Number(year) : undefined });
    }
  }
  return papers;
}

/** The byline `orx paper` prints for an OpenAlex, bioRxiv or PubMed paper:
 * `# title`, its authors, then a line starting with the publication date. */
export function paperHeader(output: string): PaperMeta | null {
  const [heading, ...rest] = output.split(/\r?\n/, 4);
  if (!heading?.startsWith("# ")) return null;
  const dated = (line: string) => /^\d{4}\b/.test(line);
  const byline = rest[0] && !dated(rest[0]) && !/^[A-Z][A-Za-z]*:\s/.test(rest[0]) ? rest[0] : null;
  const date = rest.find(dated);
  return {
    title: heading.slice(2).trim(),
    // bioRxiv separates "Family, G." names with semicolons; the others list "Given Family" with commas.
    authors: byline?.replace(/,\s*… \(\+\d+ more\)$/, "").split(byline.includes(";") ? /;\s*/ : /,\s*/).filter(Boolean),
    year: date ? Number(date.slice(0, 4)) : undefined,
  };
}

/** A name's family name: the part before a comma, else the last word. */
export function familyName(name: string): string {
  const [family, given] = name.split(",");
  return given === undefined ? name.trim().split(/\s+/).at(-1) ?? name : family.trim();
}

/** The metric a run card plots: validation before training metrics, losses
 * and error rates before anything else, then the first logged. */
export function headlineMetric(metrics: TrackioMetric[]): TrackioMetric | null {
  const score = (name: string) =>
    (/(?:^|[/_.\s-])(?:val|valid|validation|eval|test)(?:[/_.\s-]|$)/i.test(name) ? 2 : 0)
    + (/loss|bpb|perplexity|ppl|error/i.test(name) ? 1 : 0);
  return metrics.filter((metric) => metric.series.length > 1)
    .reduce<TrackioMetric | null>((best, metric) => (best === null || score(metric.name) > score(best.name) ? metric : best), null);
}

export interface CitedPaper {
  source: LitSource;
  id: string;
  /** The one-based PDF pages the links name, in order. */
  pages: number[];
  /** Whether the turn fetched the paper, rather than citing it from a search result or memory. */
  read: boolean;
  output?: string;
}

const PAPER_LINK = /https?:\/\/(?:www\.)?(alphaxiv\.org|arxiv\.org|doi\.org|openalex\.org|pubmed\.ncbi\.nlm\.nih\.gov)\/([^\s)<>\]"']+)/gi;

/** The paper a link opens, keyed as `orx paper` keys it: an arXiv id, DOI, OpenAlex id or PMID. */
function linkedPaper(host: string, path: string): { source: LitSource; id: string; page?: number } | null {
  const [route, query = ""] = path.split("?");
  const page = Number(/(?:^|&)page=(\d+)/.exec(query)?.[1]) || undefined;
  switch (host.toLowerCase()) {
    case "alphaxiv.org":
    case "arxiv.org": {
      const id = /^(?:abs|pdf|overview)\/([^/#]+)/.exec(route)?.[1]?.replace(/\.(?:pdf|md)$/i, "").replace(/v\d+$/, "");
      return id ? { source: "alphaxiv", id, page } : null;
    }
    case "doi.org": {
      let doi: string;
      try {
        doi = decodeURIComponent(route.split("#")[0]).replace(/[.,]+$/, "");
      } catch {
        return null;
      }
      return /^10\.\d+\//.test(doi) ? { source: doi.startsWith("10.1101/") ? "biorxiv" : "openalex", id: doi } : null;
    }
    case "openalex.org": {
      const id = /^(?:works\/)?(W\d+)/i.exec(route)?.[1];
      return id ? { source: "openalex", id } : null;
    }
    default: {
      const id = /^(\d{1,9})\/?$/.exec(route)?.[1];
      return id ? { source: "pubmed", id } : null;
    }
  }
}

/** The papers `markdown` links, in the order it first cites them, each with the pages it names. */
export function linkedPapers(markdown: string): Array<Omit<CitedPaper, "read" | "output">> {
  const papers = new Map<string, Omit<CitedPaper, "read" | "output">>();
  for (const match of markdown.matchAll(PAPER_LINK)) {
    const paper = linkedPaper(match[1], match[2]);
    if (!paper) continue;
    const key = paperKey(paper.source, paper.id);
    const cited = papers.get(key) ?? { source: paper.source, id: paper.id, pages: [] };
    if (paper.page && !cited.pages.includes(paper.page)) cited.pages.push(paper.page);
    papers.set(key, cited);
  }
  return [...papers.values()];
}

/** One key for a paper however it was named: arXiv ids without their version, DOIs in lower case. */
export function paperKey(source: LitSource, id: string): string {
  const linked = linkedPapers(id.trim())[0];
  const [paperSource, paperId] = linked ? [linked.source, linked.id] : [source, id];
  const bare = paperId.trim().toLowerCase().replace(/^(?:doi|arxiv|pmid):/, "");
  return paperSource === "alphaxiv" ? `arxiv:${bare.replace(/v\d+$/, "")}` : `work:${bare}`;
}
