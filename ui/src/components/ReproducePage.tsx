import { useQuery } from "@tanstack/react-query";
import { useNavigate, useRouteContext } from "@tanstack/react-router";
import { useEffect, useRef } from "react";

import { showCustomDialog } from "../dialog";
import { seedComposer } from "../composerStash";
import { m } from "../paraglide/messages.js";
import { getUiStateQuery, listProjectsQuery, resolvePaperQuery } from "../queries/projects";
import { useRuntime } from "../RemoteRuntime";
import { RouteFailure, RoutePending } from "../routePages";
import { NewProjectDialog } from "./ProjectsHome";
import { showAlert } from "./ui";

// Mirrors the backend's versionless_id/is_arxiv_id: a `v` after a dash is part of an alphaXiv name.
const VERSION_SUFFIX = /([^-])v\d+$/;
const ARXIV_ID = /^\d{4}\.\d{4,5}(v\d+)?$|\//;

/** Where an `openresearch://reproduce` link lands: a new project for the paper,
 * confirmed by the user, whose first chat opens with a reproduce prompt. */
export function ReproducePage({ paperId }: { paperId: string | null }) {
  const runtime = useRuntime();
  const navigate = useNavigate();
  const { queryClient } = useRouteContext({ from: "__root__" });
  const projectsOptions = listProjectsQuery();
  const projectsQuery = useQuery(projectsOptions);
  const stateQuery = useQuery(getUiStateQuery());
  useQuery({ ...resolvePaperQuery(paperId ?? ""), enabled: Boolean(paperId) });
  const shownPaper = useRef<string | null>(null);
  const projects = projectsQuery.data;
  const state = stateQuery.data;
  // A first launch walks through onboarding instead; the link can be opened again after.
  const onboarding = projects?.length === 0 && state?.onboardingCompleted === false;
  useEffect(() => {
    if (!paperId || onboarding) void navigate({ to: "/", replace: true });
  }, [paperId, onboarding, navigate]);
  const error = projectsQuery.error ?? stateQuery.error;
  const retry = () => { void projectsQuery.refetch(); void stateQuery.refetch(); };
  useEffect(() => {
    if (!paperId || !projects || !state || onboarding || error || shownPaper.current === paperId) return;
    shownPaper.current = paperId;
    showCustomDialog({
      component: NewProjectDialog,
      props: {
        remote: runtime.kind === "ssh",
        preferredAgent: state.preferredAgent,
        initialPaperId: paperId,
        onCreated: (project, publicationError) => {
          // The dialog lets the user switch papers or modes; only the linked paper gets the prompt.
          if (project.paperId === paperId.replace(VERSION_SUFFIX, "$1")) {
            const title = queryClient.getQueryData(resolvePaperQuery(paperId).queryKey)?.title?.trim() || project.name;
            const source = ARXIV_ID.test(paperId) ? `arXiv:${paperId}` : `alphaxiv.org/abs/${paperId}`;
            seedComposer(project.id, m.reproduce_paper_prompt({ title, source }));
          }
          queryClient.setQueryData(projectsOptions.queryKey, (cur) => [...(cur ?? []).filter((p) => p.id !== project.id), project]);
          void navigate({
            href: `/projects/${encodeURIComponent(project.id)}${publicationError ? "/settings/projects" : ""}`,
            replace: true,
          });
          if (publicationError) showAlert(publicationError, "error");
        },
      },
      onConfirm: () => {},
      onCancel: () => { void navigate({ to: "/", replace: true }); },
    });
  }, [paperId, projects, state, onboarding, error, runtime.kind, navigate, queryClient, projectsOptions.queryKey]);
  if (error) return <RouteFailure error={error} reset={retry} />;
  if (!paperId || !projects || !state || onboarding) return <RoutePending />;
  return <div className="app flex flex-col h-full bg-background" />;
}
