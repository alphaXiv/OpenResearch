import { createFileRoute } from "@tanstack/react-router";
import { ReproducePage } from "../components/ReproducePage";

// Links carry `arxiv=arXiv:<id>`: a bare id would be read as a number (2401.10000 -> 2401.1).
const ARXIV_LINK = /^arxiv:(\d{4}\.\d{4,5}(v\d+)?)$/i;

export function linkedArxivId(value: unknown): string | null {
  return typeof value === "string" ? value.match(ARXIV_LINK)?.[1] ?? null : null;
}

export const Route = createFileRoute("/reproduce")({
  // Passed through unchanged so the router never rewrites the link it was opened with.
  validateSearch: (search): { arxiv?: unknown } => ({ arxiv: search.arxiv }),
  component: function Reproduce() {
    return <ReproducePage arxiv={linkedArxivId(Route.useSearch().arxiv)} />;
  },
});
