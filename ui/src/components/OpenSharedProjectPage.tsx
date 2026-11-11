import { useNavigate } from "@tanstack/react-router";
import { useEffect, useRef } from "react";

import type { SharedSource } from "../api";
import { useRuntime } from "../RemoteRuntime";
import { RoutePending } from "../routePages";
import { useWorkspace } from "../workspace";
import { NewProjectDialog } from "./ProjectsHome";

/** Where a `.orx` file or published link opened with the desktop app lands: the new-project dialog, opening it. */
export function OpenSharedProjectPage({ file, shared }: { file: string | null; shared: string | null }) {
  const { showCustomDialog } = useWorkspace().dialogs;
  const runtime = useRuntime();
  const navigate = useNavigate();
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
    showCustomDialog({
      component: NewProjectDialog,
      props: {
        remote: runtime.kind === "ssh",
        preferredAgent: null,
        initialShared: source,
        onCreated: (project) => {
          void navigate({ href: `/projects/${encodeURIComponent(project.id)}`, replace: true });
        },
      },
      onConfirm: () => {},
      onCancel: () => { void navigate({ to: "/", replace: true }); },
    });
  }, [file, shared, runtime.kind, navigate, showCustomDialog]);
  return file || shared ? <div className="app flex flex-col h-full bg-background" /> : <RoutePending />;
}
