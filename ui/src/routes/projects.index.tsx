import { createFileRoute } from "@tanstack/react-router";
import { ProjectsPage } from "../routePages";

export const Route = createFileRoute("/projects/")({ component: ProjectsPage });
