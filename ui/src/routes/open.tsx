import { createFileRoute } from "@tanstack/react-router";
import { OpenSharedProjectPage } from "../components/OpenSharedProjectPage";

export const Route = createFileRoute("/open")({
  validateSearch: (search: Record<string, unknown>): { file?: string } =>
    typeof search.file === "string" ? { file: search.file } : {},
  component: function OpenShared() {
    return <OpenSharedProjectPage file={Route.useSearch().file ?? null} />;
  },
});
