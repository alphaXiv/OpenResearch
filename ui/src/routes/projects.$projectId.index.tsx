import { createFileRoute } from "@tanstack/react-router";
import { ResumeProject } from "../routePages";

export const Route = createFileRoute("/projects/$projectId/")({
  component: ProjectIndex,
});

function ProjectIndex() {
  const { projectId } = Route.useParams();
  return <ResumeProject projectId={projectId} />;
}
