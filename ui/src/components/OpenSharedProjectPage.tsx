import { useNavigate, useRouteContext } from "@tanstack/react-router";
import { useEffect, useRef } from "react";

import { useRuntime } from "../RemoteRuntime";
import { RoutePending } from "../routePages";
import { useWorkspace } from "../workspace";
import { NewProjectDialog } from "./ProjectsHome";

/** Where a `.orx` file opened with the desktop app lands: the new-project dialog, opening that file. */
export function OpenSharedProjectPage({ file }: { file: string | null }) {
  const { dialogs: { showCustomDialog }, queries: { listProjectsQuery } } = useWorkspace();
  const runtime = useRuntime();
  const navigate = useNavigate();
  const { queryClient } = useRouteContext({ from: "__root__" });
  const shown = useRef<string | null>(null);
  useEffect(() => {
    if (!file) {
      void navigate({ to: "/", replace: true });
      return;
    }
    if (shown.current === file) return;
    shown.current = file;
    const projectsKey = listProjectsQuery().queryKey;
    showCustomDialog({
      component: NewProjectDialog,
      props: {
        remote: runtime.kind === "ssh",
        preferredAgent: null,
        initialShared: file,
        onCreated: (project) => {
          queryClient.setQueryData(projectsKey, (cur) => [...(cur ?? []).filter((p) => p.id !== project.id), project]);
          void navigate({ href: `/projects/${encodeURIComponent(project.id)}`, replace: true });
        },
      },
      onConfirm: () => {},
      onCancel: () => { void navigate({ to: "/", replace: true }); },
    });
  }, [file, runtime.kind, navigate, queryClient, showCustomDialog, listProjectsQuery]);
  return file ? <div className="app flex flex-col h-full bg-background" /> : <RoutePending />;
}
