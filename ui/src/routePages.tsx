import { Link, useNavigate, type ErrorComponentProps } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import {
  getUiState,
  listProjects,
  type Project,
  type UiState,
} from "./api";
import { useRuntime } from "./RemoteRuntime";
import { useOrxEvents } from "./events";
import { clearReadDemoSessions } from "./demoSessionState";
import { globalResumeLocation, projectResumeLocation } from "./routeResume";
import { getRememberedGlobalWorkspace, globalWorkspaceWriter } from "./workspacePersistence";
import { m } from "./paraglide/messages.js";
import { Onboarding } from "./components/Onboarding";
import { ProjectsHome } from "./components/ProjectsHome";
import { OfflineBanner } from "./components/OfflineBanner";
import { RemoteStatus } from "./components/RemoteStatus";
import { UpdateBanner, useUpdateStatus } from "./components/UpdateBanner";
import { Button, showAlert, Spinner } from "./components/ui";

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
  const navigate = useNavigate();
  const [error, setError] = useState<Error | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let current = true;
    setError(null);
    void (projectId ? projectResumeLocation(projectId) : globalResumeLocation())
      .then((href) => { if (current) void navigate({ href, replace: true }); })
      .catch((cause: unknown) => {
        if (current) setError(cause instanceof Error ? cause : new Error(String(cause)));
      });
    return () => { current = false; };
  }, [projectId, attempt, navigate]);
  return error ? <RouteFailure error={error} reset={() => setAttempt((value) => value + 1)} /> : <RoutePending />;
}

export function ResumeGlobal() { return <Resume />; }
export function ResumeProject({ projectId }: { projectId: string }) { return <Resume projectId={projectId} />; }

export function ProjectsPage() {
  const runtime = useRuntime();
  const navigate = useNavigate();
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [state, setState] = useState<UiState | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const [attempt, setAttempt] = useState(0);
  const { status } = useUpdateStatus(runtime.kind === "local");
  useEffect(() => {
    let current = true;
    setError(null);
    document.title = "OpenResearch";
    void Promise.all([listProjects(), getUiState()]).then(([loadedProjects, loadedState]) => {
      if (!current) return;
      setProjects(loadedProjects);
      setState(loadedState);
      globalWorkspaceWriter.queue({
        ...(getRememberedGlobalWorkspace() ?? loadedState.workspace ?? { railOpen: true, panelWidth: 760, experimentsView: "table" }),
        lastLocation: "/projects",
      });
    }).catch((cause: unknown) => {
      if (current) setError(cause instanceof Error ? cause : new Error(String(cause)));
    });
    return () => { current = false; };
  }, [attempt]);
  useOrxEvents({
    onRun: () => {},
    onExperiment: () => {},
    onProject: (project) => setProjects((current) => current && [...current.filter((item) => item.id !== project.id), project]),
    onReconnect: () => setAttempt((value) => value + 1),
  });
  const openProject = (projectId: string) => void navigate({ to: "/projects/$projectId", params: { projectId } });

  return (
    <div className="app flex flex-col h-full">
      {runtime.kind === "local" && <><OfflineBanner /><UpdateBanner status={status} /></>}
      {error ? <RouteFailure error={error} reset={() => setAttempt((value) => value + 1)} />
        : projects === null || state === null ? <RoutePending />
        : projects.length === 0 && !state.onboardingCompleted ? (
          <Onboarding
            preferredAgent={state.preferredAgent}
            onDone={(project) => {
              clearReadDemoSessions();
              openProject(project.id);
            }}
          />
        ) : (
          <ProjectsHome
            remote={runtime.kind === "ssh"}
            projects={projects}
            onOpen={openProject}
            onCreated={(project, publicationError) => {
              if (publicationError) {
                showAlert(publicationError, "error");
                void navigate({ to: "/projects/$projectId/settings/$tab", params: { projectId: project.id, tab: "git" } });
              } else openProject(project.id);
            }}
            onDeleted={(id) => setProjects((current) => current?.filter((project) => project.id !== id) ?? null)}
          />
        )}
      {runtime.kind === "ssh" && <RemoteStatus runtime={runtime} corner />}
    </div>
  );
}
