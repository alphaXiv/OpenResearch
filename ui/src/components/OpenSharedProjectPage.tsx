import { useNavigate, useRouteContext } from "@tanstack/react-router";
import { useEffect, useRef } from "react";

import type { SharedSource } from "../api";
import { useRuntime } from "../RemoteRuntime";
import { RoutePending } from "../routePages";
import { useWorkspace } from "../workspace";
import { NewProjectDialog } from "./ProjectsHome";

/** Where a `.orx` file or published link opened with the desktop app lands: the new-project dialog, opening it. */
export function OpenSharedProjectPage({ file, shared }: { file: string | null; shared: string | null }) {
  const { dialogs: { showCustomDialog }, queries: { listProjectsQuery } } = useWorkspace();
  const runtime = useRuntime();
  const navigate = useNavigate();
  const { queryClient } = useRouteContext({ from: "__root__" });
  const shown = useRef<string | null>(null);
  useEffect(() => {
    const source: SharedSource | null = file ?? (shared ? { link: shared } : null);
    if (!source) {
      void navigate({ to: "/", replace: true });
      return;
    }
    const key = file ?? `link:${shared}`;
    if (shown.current === key) return;
    shown.current = key;
    const projectsKey = listProjectsQuery().queryKey;
    showCustomDialog({
      component: NewProjectDialog,
      props: {
        remote: runtime.kind === "ssh",
        preferredAgent: null,
        initialShared: source,
        onCreated: (project) => {
          queryClient.setQueryData(projectsKey, (cur) => [...(cur ?? []).filter((p) => p.id !== project.id), project]);
          void navigate({ href: `/projects/${encodeURIComponent(project.id)}`, replace: true });
        },
      },
      onConfirm: () => {},
      onCancel: () => { void navigate({ to: "/", replace: true }); },
    });
  }, [file, shared, runtime.kind, navigate, queryClient, showCustomDialog, listProjectsQuery]);
  return file || shared ? <div className="app flex flex-col h-full bg-background" /> : <RoutePending />;
}
