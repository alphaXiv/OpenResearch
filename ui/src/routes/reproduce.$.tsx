import { createFileRoute } from "@tanstack/react-router";
import { ReproducePage } from "../components/ReproducePage";

// An arXiv id (`2401.10000`, `hep-th/9901001`) or an alphaXiv-exclusive one (`2610.some-slug`).
const PAPER_ID = /^[\w.-]+(\/[\w.-]+)?$/;

export function linkedPaperId(splat: string | undefined): string | null {
  return splat && splat.length <= 128 && PAPER_ID.test(splat) ? splat : null;
}

// The id rides in the path: the router would read `?id=2401.10000` as the number 2401.1.
export const Route = createFileRoute("/reproduce/$")({
  component: function Reproduce() {
    return <ReproducePage paperId={linkedPaperId(Route.useParams()._splat)} />;
  },
});
