import { isCancelledError, useQuery } from "@tanstack/react-query";
import { listProjectsQuery, getUiStateQuery } from "./queries/projects";
import { useRouteContext, Link, useNavigate, type ErrorComponentProps } from "@tanstack/react-router";
import { useEffect, useState } from "react";

import { useRuntime } from "./RemoteRuntime";

import { clearReadDemoSessions } from "./demoSessionState";
import { globalResumeLocation, projectResumeLocation } from "./routeResume";
import { m } from "./paraglide/messages.js";
import { Onboarding } from "./components/Onboarding";
import { OfflineBanner } from "./components/OfflineBanner";
import { WorkspaceConnection } from "./components/WorkspaceConnection";
import { UpdateBanner, useUpdateStatus } from "./components/UpdateBanner";
import { DesktopAppBanner } from "./components/DesktopAppBanner";
import { Button, Spinner } from "./components/ui";

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
  const { queryClient: client } = useRouteContext({ from: "__root__" });
  const navigate = useNavigate();
  const [error, setError] = useState<Error | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let current = true;
    setError(null);
    void (projectId ? projectResumeLocation(projectId, client) : globalResumeLocation(client))
      .then((href) => { if (current) void navigate({ href, replace: true }); })
      .catch((cause: unknown) => {
        if (!current) return;
        // A shared read can be cancelled by invalidation or the last observer leaving.
        if (isCancelledError(cause)) setAttempt((value) => value + 1);
        else setError(cause instanceof Error ? cause : new Error(String(cause)));
      });
    return () => { current = false; };
  }, [projectId, attempt, navigate, client]);
  return error ? <RouteFailure error={error} reset={() => setAttempt((value) => value + 1)} /> : <RoutePending />;
}

export function ResumeGlobal() { return <Resume />; }
export function ResumeProject({ projectId }: { projectId: string }) { return <Resume projectId={projectId} />; }

export function ProjectsPage() {
  const runtime = useRuntime();
  const navigate = useNavigate();
  const projectsOptions = listProjectsQuery();
  const projectsQuery = useQuery(projectsOptions);
  const stateQuery = useQuery(getUiStateQuery());
  const projects = projectsQuery.data;
  const state = stateQuery.data;
  const onboarding = projects?.length === 0;
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
          : onboarding ? (
            <Onboarding
              remote={runtime.kind === "ssh"}
              preferredAgent={state.preferredAgent}
              onDone={(project) => {
                clearReadDemoSessions();
                openProject(project.id);
              }}
            />
          ) : (
            <ResumeGlobal />
          )}
      {(runtime.kind === "ssh" || (projects && state && !onboarding)) && <WorkspaceConnection runtime={runtime} corner />}
    </div>
  );
}
