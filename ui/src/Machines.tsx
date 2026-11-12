import { WorkspaceDomContext } from "./WorkspaceDom";
import { Activity, useCallback, useEffect, useRef, useState } from "react";
import { QueryClientProvider, useQuery, type QueryClient } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import type { RemoteSessionInfo } from "./api";
import { createWorkspace, WorkspaceContext } from "./workspace";
import { createQueryClient } from "./queries/client";
import { createWorkspaceRouter } from "./router";
import { MachineSelectionContext } from "./machineSelection";
import { askBeforeClosing } from "./closeDialog";
import { QueryDevtools } from "./components/QueryDevtools";
import { Button, showAlert, Spinner } from "./components/ui";
import { m } from "./paraglide/messages.js";

function createMachine(queryClient: QueryClient, id: string, initialLocation = "/") {
  const workspace = createWorkspace(
    `${id}:${crypto.randomUUID()}`,
    id === "local" ? "" : `/_orx/workspaces/${encodeURIComponent(id)}`,
    queryClient,
  );
  return {
    id,
    workspace,
    router: createWorkspaceRouter(workspace, initialLocation),
  };
}

type Machine = ReturnType<typeof createMachine>;

// A file the desktop app was asked to open is on this computer, whichever machine is active.
const isSharedFileOpen = (href: string) => href.startsWith("/open?");

function browserLocation() {
  const target = new URL(location.href);
  target.searchParams.delete("machine");
  return target.pathname + target.search + target.hash;
}

function WorkspaceView({
  machine,
  active,
  replace,
}: {
  machine: Machine;
  active: boolean;
  replace: () => void;
}) {
  const { workspace, router, id } = machine;
  const { DialogManager } = workspace.dialogs;
  const element = useRef<HTMLDivElement>(null);
  const [dom] = useState(() => ({
    portalContainer: document.createElement("div"),
    id: (name: string) => `${workspace.identity}-${name}`,
  }));
  const attachPortals = useCallback(
    (element: HTMLDivElement | null) => {
      element?.append(dom.portalContainer);
    },
    [dom],
  );
  const visited = useRef(false);

  useEffect(
    () => workspace.subscribeReplacement(replace),
    [workspace, replace],
  );
  useEffect(() => {
    if (!active) return;
    const publish = (replace: boolean) => {
      const target = new URL(router.history.location.href, location.href);
      target.searchParams.delete("machine");
      if (id !== "local") target.searchParams.set("machine", id);
      if (target.href !== location.href)
        window.history[replace ? "replaceState" : "pushState"](
          { machine: id },
          "",
          target,
        );
    };
    publish(false);
    const unsubscribe = router.history.subscribe(({ action }) =>
      publish(action.type !== "PUSH"),
    );
    const openPath = (event: Event) => {
      if (event instanceof CustomEvent && typeof event.detail === "string" && !isSharedFileOpen(event.detail))
        void router.navigate({ href: event.detail });
    };
    window.addEventListener("orx:open-path", openPath);
    let observer: MutationObserver | undefined;
    const focus = () => {
      if (element.current?.contains(document.activeElement)) {
        observer?.disconnect();
        return;
      }
      const trigger = element.current?.querySelector<HTMLElement>(
        "[data-machine-switcher]",
      );
      if (trigger) {
        trigger.focus({ preventScroll: true });
        observer?.disconnect();
      }
    };
    const frame = requestAnimationFrame(() => {
      if (visited.current || id !== "local") {
        observer = new MutationObserver(focus);
        if (element.current)
          observer.observe(element.current, { childList: true, subtree: true });
        focus();
      }
      visited.current = true;
    });
    return () => {
      unsubscribe();
      observer?.disconnect();
      cancelAnimationFrame(frame);
      window.removeEventListener("orx:open-path", openPath);
    };
  }, [active, router, id]);

  return (
    <WorkspaceContext value={workspace}>
      <WorkspaceDomContext value={dom}>
        <div
          ref={element}
          className="h-full"
          data-machine={id}
          hidden={!active}
          inert={!active}
        >
          <Activity mode={active ? "visible" : "hidden"}>
            <RouterProvider router={router} />
            <DialogManager />
            {import.meta.env.DEV && <QueryDevtools />}
          </Activity>
          <div ref={attachPortals} />
        </div>
      </WorkspaceDomContext>
    </WorkspaceContext>
  );
}

function MachineWorkspaces({
  local,
  replaceLocal,
}: {
  local: Machine;
  replaceLocal: () => void;
}) {
  const owner = local.workspace;
  const [workspaces, setWorkspaces] = useState(
    () => new Map([["local", local]]),
  );
  const [activeId, setActiveId] = useState("local");
  const [requested] = useState(() =>
    new URL(location.href).searchParams.get("machine"),
  );
  const [restoring, setRestoring] = useState(Boolean(requested));
  useEffect(() => {
    const flush = () => {
      for (const { workspace } of workspaces.values())
        void workspace.persistence.globalWorkspaceWriter.flush(true);
    };
    window.addEventListener("pagehide", flush);
    return () => window.removeEventListener("pagehide", flush);
  }, [workspaces]);
  const ownerRuntime = useQuery(owner.queries.getRuntimeQuery());
  const canSwitch = ownerRuntime.data?.kind === "local";
  const sessionsQuery = useQuery({
    ...owner.queries.listRemoteSessionsQuery(),
    enabled: canSwitch,
    refetchInterval: 2_000,
    refetchIntervalInBackground: true,
  });
  const sessions = sessionsQuery.data ?? [];
  const workspacesRef = useRef(workspaces);
  workspacesRef.current = workspaces;

  function selectRemote(session: RemoteSessionInfo) {
    if (session.status === "disconnected" && session.error === null && !session.canStartNewHost) {
      void owner.api.reconnectRemoteSession(session.id).catch((error: unknown) =>
        showAlert(error instanceof Error ? error.message : String(error), "error"),
      );
    }
    setWorkspaces((current) =>
      current.has(session.id)
        ? current
        : new Map(current).set(session.id, createMachine(owner.queryClient, session.id)),
    );
    setActiveId(session.id);
  }

  useEffect(() => {
    const openShared = (event: Event) => {
      if (!(event instanceof CustomEvent) || typeof event.detail !== "string" || !isSharedFileOpen(event.detail)) return;
      setActiveId("local");
      void local.router.navigate({ href: event.detail });
    };
    window.addEventListener("orx:open-path", openShared);
    return () => window.removeEventListener("orx:open-path", openShared);
  }, [local]);

  useEffect(() => {
    // The window's pending work is the local machine's, whichever machine is showing.
    const closeRequest = (event: Event) => {
      if (!(event instanceof CustomEvent)) return;
      const showing = workspaces.get(activeId) ?? local;
      askBeforeClosing(showing.workspace.dialogs, event.detail, {
        machine: local.workspace,
        open: (href) => {
          setActiveId("local");
          void local.router.navigate({ href });
        },
      });
    };
    window.addEventListener("orx:close-request", closeRequest);
    return () => window.removeEventListener("orx:close-request", closeRequest);
  }, [activeId, workspaces, local]);

  useEffect(() => {
    setWorkspaces((current) =>
      current.get("local") === local
        ? current
        : new Map(current).set("local", local),
    );
  }, [local]);

  useEffect(() => {
    if (!sessionsQuery.data) return;
    const live = new Set(sessionsQuery.data.map((session) => session.id));
    setWorkspaces((current) => {
      const kept = new Map(
        [...current].filter(([id]) => id === "local" || live.has(id)),
      );
      return kept.size === current.size ? current : kept;
    });
    setActiveId((id) => (id === "local" || live.has(id) ? id : "local"));
  }, [sessionsQuery.data]);

  useEffect(() => {
    if (!restoring || !sessionsQuery.data || !requested) return;
    const session = sessionsQuery.data.find((entry) => entry.id === requested);
    if (session) {
      setWorkspaces((current) =>
        new Map(current).set(
          session.id,
          createMachine(owner.queryClient, session.id, browserLocation()),
        ),
      );
      setActiveId(session.id);
      setRestoring(false);
    }
  }, [requested, restoring, sessionsQuery.data]);

  useEffect(() => {
    const pop = () => {
      const id = new URL(location.href).searchParams.get("machine") ?? "local";
      const machine = workspacesRef.current.get(id);
      if (!machine) return;
      void machine.router.navigate({ href: browserLocation(), replace: true });
      setActiveId(id);
    };
    window.addEventListener("popstate", pop);
    return () => window.removeEventListener("popstate", pop);
  }, []);

  if (restoring)
    return (
      <main className="app flex h-full flex-col items-center justify-center gap-3 bg-background text-text">
        {ownerRuntime.isPending || (canSwitch && sessionsQuery.isPending) ? (
          <Spinner />
        ) : (
          <>
            <p role="alert">
              {sessionsQuery.error?.message ??
                ownerRuntime.error?.message ??
                m.remote_machine_disconnected()}
            </p>
            <Button
              onClick={() => {
                window.history.replaceState(null, "", "/");
                setRestoring(false);
              }}
            >
              {m.projects_local()}
            </Button>
          </>
        )}
      </main>
    );

  return (
    <MachineSelectionContext
      value={{
        owner,
        activeId,
        sessions,
        canSwitch,
        selectLocal: () => setActiveId("local"),
        selectRemote,
      }}
    >
      {[...workspaces].map(([id, machine]) => (
        <WorkspaceView
          key={machine.workspace.identity}
          machine={machine}
          active={id === activeId}
          replace={
            id === "local"
              ? replaceLocal
              : () =>
                  setWorkspaces((current) =>
                    new Map(current).set(id, createMachine(owner.queryClient, id)),
                  )
          }
        />
      ))}
    </MachineSelectionContext>
  );
}

export function Machines() {
  const [local, setLocal] = useState(() =>
    createMachine(
      createQueryClient(),
      "local",
      new URL(location.href).searchParams.has("machine")
        ? "/"
        : browserLocation(),
    ),
  );
  return (
    <QueryClientProvider client={local.workspace.queryClient}>
      <MachineWorkspaces
        local={local}
        replaceLocal={() => setLocal((current) => createMachine(current.workspace.queryClient, "local"))}
      />
    </QueryClientProvider>
  );
}
