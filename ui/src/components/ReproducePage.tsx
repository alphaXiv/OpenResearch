import { useQuery } from "@tanstack/react-query";
import { useNavigate, useRouteContext } from "@tanstack/react-router";
import { useEffect } from "react";

import { seedComposer } from "../composerStash";
import { m } from "../paraglide/messages.js";
import { getUiStateQuery, listProjectsQuery, resolvePaperQuery } from "../queries/projects";
import { useRuntime } from "../RemoteRuntime";
import { RouteFailure, RoutePending } from "../routePages";
import { NewProjectDialog } from "./ProjectsHome";
import { showAlert } from "./ui";

/** Where an `openresearch://reproduce` link lands: a new project for the paper,
 * confirmed by the user, whose first chat opens with a reproduce prompt. */
export function ReproducePage({ arxiv }: { arxiv: string | null }) {
  const runtime = useRuntime();
  const navigate = useNavigate();
  const { queryClient } = useRouteContext({ from: "__root__" });
  const projectsOptions = listProjectsQuery();
  const projectsQuery = useQuery(projectsOptions);
  const stateQuery = useQuery(getUiStateQuery());
  const paperQuery = useQuery({ ...resolvePaperQuery(arxiv ?? ""), enabled: Boolean(arxiv) });
  const projects = projectsQuery.data;
  const state = stateQuery.data;
  // A first launch walks through onboarding instead; the link can be opened again after.
  const onboarding = projects?.length === 0 && state?.onboardingCompleted === false;
  useEffect(() => {
    if (!arxiv || onboarding) void navigate({ to: "/", replace: true });
  }, [arxiv, onboarding, navigate]);
  const error = projectsQuery.error ?? stateQuery.error;
  const retry = () => { void projectsQuery.refetch(); void stateQuery.refetch(); };
  if (error) return <RouteFailure error={error} reset={retry} />;
  if (!arxiv || !projects || !state || onboarding) return <RoutePending />;
  return (
    <div className="app flex flex-col h-full bg-background">
      <NewProjectDialog
        key={arxiv}
        remote={runtime.kind === "ssh"}
        preferredAgent={state.preferredAgent}
        initialPaperId={arxiv}
        onClose={() => void navigate({ to: "/", replace: true })}
        onCreated={(project, publicationError) => {
          // The dialog lets the user switch papers or modes; only the linked paper gets the prompt.
          if (project.paperId === arxiv.replace(/v\d+$/, "")) {
            const title = paperQuery.data?.title?.trim() || project.name;
            seedComposer(project.id, m.reproduce_paper_prompt({ title, arxiv }));
          }
          queryClient.setQueryData(projectsOptions.queryKey, (cur) => [...(cur ?? []).filter((p) => p.id !== project.id), project]);
          void navigate({
            href: `/projects/${encodeURIComponent(project.id)}${publicationError ? "/settings/projects" : ""}`,
            replace: true,
          });
          if (publicationError) showAlert(publicationError, "error");
        }}
      />
    </div>
  );
}
