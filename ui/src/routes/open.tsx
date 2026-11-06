import { createFileRoute } from "@tanstack/react-router";
import { OpenSharedProjectPage } from "../components/OpenSharedProjectPage";

export const Route = createFileRoute("/open")({
  validateSearch: (search: Record<string, unknown>): { file?: string; shared?: string } => ({
    ...(typeof search.file === "string" ? { file: search.file } : {}),
    ...(typeof search.shared === "string" ? { shared: search.shared } : {}),
  }),
  component: function OpenShared() {
    const { file, shared } = Route.useSearch();
    return <OpenSharedProjectPage file={file ?? null} shared={shared ?? null} />;
  },
});
