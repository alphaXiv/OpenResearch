// arXiv and Semantic Scholar discovery sources, owned by the tuandang304 fork.
// orxCommand.ts and LitSourceLogo.tsx spread these into the upstream source
// maps; see docs/fork-maintenance.md.
import arxivSvg from "../../assets/lit-sources/arxiv.svg?raw";
import semanticScholarSvg from "../../assets/lit-sources/semanticscholar.svg?raw";

export type ForkLitSource = "arxiv" | "semanticscholar";

export const FORK_LIT_SOURCES: ForkLitSource[] = ["arxiv", "semanticscholar"];

export const FORK_LIT_SOURCE_NAME: Record<ForkLitSource, string> = {
  arxiv: "arXiv",
  semanticscholar: "Semantic Scholar",
};

export const FORK_LIT_SOURCE_SVG: Record<ForkLitSource, string> = {
  arxiv: arxivSvg,
  semanticscholar: semanticScholarSvg,
};
