import { useWorkspace } from "./workspace";
import { isCancelledError, useQuery } from "@tanstack/react-query";

import { useRouteContext, Link, useNavigate, type ErrorComponentProps } from "@tanstack/react-router";
import { useEffect, useState } from "react";

import { useRuntime } from "./RemoteRuntime";

import { clearReadDemoSessions } from "./demoSessionState";
import { globalResumeLocation, projectResumeLocation } from "./routeResume";
import { m } from "./paraglide/messages.js";
import { Onboarding } from "./components/Onboarding";
import { ProjectsEmptyState } from "./components/ProjectsHome";
import { OfflineBanner } from "./components/OfflineBanner";
import { WorkspaceConnection } from "./components/WorkspaceConnection";
import { UpdateBanner, useUpdateStatus } from "./components/UpdateBanner";
import { DesktopAppBanner } from "./components/DesktopAppBanner";
import { Button, Spinner, showAlert } from "./components/ui";

export function RoutePending() {
  return <div className="flex flex-1 h-full items-center justify-center"><Spinner /></div>;
}

export function RouteNotFound() {
  return (
    <div className="flex flex-1 h-full flex-col items-center justify-center gap-3 text-subtext">
      <p>{m.model_picker_unavailable()}</p>
      <Link to="/projects">{m.app_projects()}</Link>
    </div>
  );
}

export function RouteFailure({ error, reset }: Pick<ErrorComponentProps, "error" | "reset">) {
  return (
    <div className="flex flex-1 h-full flex-col items-center justify-center gap-3 text-subtext">
      <p role="alert">{error.message}</p>
      <Button onClick={reset}>{m.app_retry()}</Button>
      <Link to="/projects">{m.app_projects()}</Link>
    </div>
  );
}

function Resume({ projectId }: { projectId?: string }) {
  const runtime = useRuntime();
  const workspace = useWorkspace();
  const navigate = useNavigate();
  const [error, setError] = useState<Error | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let current = true;
    setError(null);
    void (projectId ? projectResumeLocation(projectId, workspace) : globalResumeLocation(workspace))
      .then((href) => { if (current) void navigate({ href, replace: true }); })
      .catch((cause: unknown) => {
        if (!current) return;
        // A shared read can be cancelled by invalidation or the last observer leaving.
        if (isCancelledError(cause)) setAttempt((value) => value + 1);
        else setError(cause instanceof Error ? cause : new Error(String(cause)));
      });
    return () => { current = false; };
  }, [projectId, attempt, navigate, workspace]);
  return <>{error ? <RouteFailure error={error} reset={() => setAttempt((value) => value + 1)} /> : <RoutePending />}{error && <WorkspaceConnection runtime={runtime} corner />}</>;
}

export function ResumeGlobal() { return <Resume />; }
export function ResumeProject({ projectId }: { projectId: string }) { return <Resume projectId={projectId} />; }

export function ProjectsPage() {
  const { listProjectsQuery, getUiStateQuery } = useWorkspace().queries;

  const runtime = useRuntime();
  const navigate = useNavigate();
  const { queryClient } = useRouteContext({ from: "__root__" });
  const projectsOptions = listProjectsQuery();
  const projectsQuery = useQuery(projectsOptions);
  const stateQuery = useQuery(getUiStateQuery());
  const projects = projectsQuery.data;
  const state = stateQuery.data;
  const noProjects = projects?.length === 0;
  const onboarded = state?.onboardingCompleted ?? false;
  // The walkthrough is a one-time setup, so the persisted flag gates it and not
  // the project count: onboarding installs a demo project, and counting projects
  // replayed the whole walkthrough the moment that demo was deleted.
  const onboarding = runtime.kind === "local" && noProjects && !onboarded;
  const agentSetup = runtime.kind === "ssh" && noProjects && !state?.preferredAgent;
  const error = projectsQuery.error ?? stateQuery.error;
  const retry = () => { void projectsQuery.refetch(); void stateQuery.refetch(); };
  const { status } = useUpdateStatus(runtime.kind === "local");
  useEffect(() => {
    document.title = "OpenResearch";
  }, []);
  const openProject = (projectId: string) => void navigate({ to: "/projects/$projectId", params: { projectId } });

  return (
    <div className="app flex flex-col h-full">
      {runtime.kind === "local" && <><OfflineBanner /><UpdateBanner status={status} /></>}
      <DesktopAppBanner />
      {error && (!projects || !state) ? <RouteFailure error={error} reset={retry} />
        : !projects || !state ? <RoutePending />
          : agentSetup && runtime.kind === "ssh" ? (
            <Onboarding remote host={runtime.session.host} preferredAgent={state.preferredAgent}
              onDone={(next) => queryClient.setQueryData(getUiStateQuery().queryKey, next)} />
          ) : onboarding ? (
            <Onboarding
              remote={false}
              preferredAgent={state.preferredAgent}
              onDone={(project) => {
                clearReadDemoSessions();
                openProject(project.id);
              }}
            />
          ) : noProjects ? (
            <ProjectsEmptyState
              remote={runtime.kind === "ssh"}
              preferredAgent={state.preferredAgent}
              onCreated={(project, publicationError) => {
                void navigate({
                  href: `/projects/${encodeURIComponent(project.id)}${publicationError ? "/settings/projects" : ""}`,
                });
                if (publicationError) showAlert(publicationError, "error");
              }}
            />
          ) : (
            <ResumeGlobal />
          )}
      {(agentSetup || (error && (!projects || !state))) && <WorkspaceConnection runtime={runtime} corner />}
    </div>
  );
}
